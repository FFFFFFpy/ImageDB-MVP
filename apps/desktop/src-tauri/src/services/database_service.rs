use crate::domain::{
    ConnectionConfig, DatabaseMode, DatabaseState, DatabaseStatus, ExternalCheckResult,
    ExternalMigrationProgress, ExternalMigrationResult, ExternalPreflightCheck, ManagedDbConfig,
    TableRowCount, TlsMode,
};
use crate::error::AppError;
use crate::infrastructure::postgres::{connect_external, MigrationRunner, PostgresManager};
use crate::infrastructure::secrets::{external_profile_key, CredentialStore};
use crate::infrastructure::settings::AppSettings;
use crate::infrastructure::settings::SettingsStore;
use chrono::Utc;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout, Duration};

#[derive(Clone)]
pub struct DatabaseService {
    postgres_manager: Arc<Mutex<PostgresManager>>,
    settings: Arc<Mutex<SettingsStore>>,
    credentials: Arc<CredentialStore>,
}

const MIGRATION_TABLES: &[&str] = &[
    "app_meta",
    "library_roots",
    "import_runs",
    "import_albums",
    "import_images",
    "library_albums",
    "library_images",
    "duplicate_candidates",
    "review_decisions",
    "file_transactions",
    "file_operations",
    "audit_events",
    "import_plans",
    "import_plan_albums",
    "import_plan_images",
    "source_album_snapshots",
    "source_album_snapshot_files",
];

const MIGRATION_SCHEMA_KINDS: &[&str] = &["constraints", "indexes"];

impl DatabaseService {
    pub fn new(
        postgres_manager: Arc<Mutex<PostgresManager>>,
        settings: Arc<Mutex<SettingsStore>>,
        credentials: Arc<CredentialStore>,
    ) -> Self {
        Self {
            postgres_manager,
            settings,
            credentials,
        }
    }

    fn connection_config_from_settings_without_secret(
        settings: &AppSettings,
    ) -> Option<ConnectionConfig> {
        let host = settings.external_host.clone()?;
        let username = settings.external_username.clone().unwrap_or_default();
        Some(ConnectionConfig {
            host,
            port: settings.external_port.unwrap_or(5432),
            database: settings
                .external_database
                .clone()
                .unwrap_or_else(|| "imagedb".to_string()),
            username,
            password: None,
            tls_mode: settings
                .external_tls_mode
                .as_deref()
                .and_then(TlsMode::from_str_opt)
                .unwrap_or_default(),
            ca_cert_path: settings.external_ca_cert_path.clone(),
            client_cert_path: settings.external_client_cert_path.clone(),
            client_key_path: settings.external_client_key_path.clone(),
            connect_timeout_secs: settings.external_connect_timeout_secs.unwrap_or(10),
            query_timeout_secs: settings.external_query_timeout_secs.unwrap_or(15),
            profile_name: settings.external_profile_name.clone(),
        })
    }

    fn connection_config_from_settings(
        settings: &AppSettings,
        credentials: &CredentialStore,
    ) -> Result<Option<ConnectionConfig>, AppError> {
        let Some(mut config) = Self::connection_config_from_settings_without_secret(settings)
        else {
            return Ok(None);
        };

        let key = external_profile_key(
            &config.host,
            config.port,
            &config.database,
            &config.username,
        );
        config.password = credentials.load(&key)?;
        Ok(Some(config))
    }

    fn with_stored_password(
        &self,
        config: &ConnectionConfig,
    ) -> Result<ConnectionConfig, AppError> {
        let mut effective = config.clone();
        if effective.password.is_none() {
            let key = external_profile_key(
                &effective.host,
                effective.port,
                &effective.database,
                &effective.username,
            );
            effective.password = self.credentials.load(&key)?;
        }
        Ok(effective)
    }

    fn redacted_config(config: &ConnectionConfig) -> ConnectionConfig {
        let mut redacted = config.clone();
        redacted.password = None;
        redacted
    }

    async fn store_and_activate_external_profile(
        &self,
        config: &ConnectionConfig,
    ) -> Result<(), AppError> {
        let mut settings = self.settings.lock().await;
        let current = settings.get().clone();
        if let Some(password) = &config.password {
            let key = external_profile_key(
                &config.host,
                config.port,
                &config.database,
                &config.username,
            );
            self.credentials.store(&key, password)?;
        }
        settings.update(crate::infrastructure::settings::AppSettings {
            database_mode: Some("external".to_string()),
            external_host: Some(config.host.clone()),
            external_port: Some(config.port),
            external_database: Some(config.database.clone()),
            external_username: Some(config.username.clone()),
            external_tls_mode: Some(config.tls_mode.as_str().to_string()),
            external_ca_cert_path: config.ca_cert_path.clone(),
            external_client_cert_path: config.client_cert_path.clone(),
            external_client_key_path: config.client_key_path.clone(),
            external_connect_timeout_secs: Some(config.connect_timeout_secs),
            external_query_timeout_secs: Some(config.query_timeout_secs),
            external_profile_name: config.profile_name.clone(),
            first_run_completed: true,
            ..current
        })?;

        let mut mgr = self.postgres_manager.lock().await;
        mgr.use_external_profile(config.clone());
        Ok(())
    }

    async fn external_target_has_rows(client: &tokio_postgres::Client) -> Result<bool, AppError> {
        for table in MIGRATION_TABLES {
            let exists = client
                .query_one(
                    "SELECT to_regclass($1) IS NOT NULL",
                    &[&format!("public.{table}")],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to inspect external table {table}: {e}"))
                })?
                .get::<_, bool>(0);
            if exists {
                let count = Self::count_table_rows(client, table).await?;
                if count > 0 {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    async fn count_table_rows(
        client: &tokio_postgres::Client,
        table: &str,
    ) -> Result<i64, AppError> {
        let sql = format!("SELECT COUNT(*)::BIGINT FROM {table}");
        client
            .query_one(&sql, &[])
            .await
            .map(|row| row.get::<_, i64>(0))
            .map_err(|e| AppError::Internal(format!("failed to count table {table}: {e}")))
    }

    async fn compare_migration_row_counts(
        managed: &tokio_postgres::Client,
        external: &tokio_postgres::Client,
    ) -> Result<Vec<TableRowCount>, AppError> {
        let mut row_counts = Vec::new();
        for table in MIGRATION_TABLES {
            let managed_rows = Self::count_table_rows(managed, table).await?;
            let external_rows = Self::count_table_rows(external, table).await?;
            row_counts.push(TableRowCount {
                table: (*table).to_string(),
                managed_rows,
                external_rows,
                matches: managed_rows == external_rows,
            });
        }
        Ok(row_counts)
    }

    fn quote_ident(ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn create_database_sql(database: &str, owner: &str) -> String {
        format!(
            "CREATE DATABASE {} OWNER {} ENCODING 'UTF8' TEMPLATE template0",
            Self::quote_ident(database),
            Self::quote_ident(owner)
        )
    }

    async fn ensure_external_database_exists(
        config: &ConnectionConfig,
        diagnostics: &mut Vec<String>,
    ) -> Result<bool, AppError> {
        if let Ok((_, handle)) = connect_external(config).await {
            handle.abort();
            diagnostics.push(format!(
                "External database '{}' already exists and is reachable",
                config.database
            ));
            return Ok(false);
        }

        diagnostics.push(format!(
            "External database '{}' is not reachable yet; checking maintenance database for creation",
            config.database
        ));

        let maintenance_databases = ["postgres", "template1"];
        let mut last_error = None;
        for database in maintenance_databases {
            if database == config.database {
                continue;
            }

            let mut maintenance_config = config.clone();
            maintenance_config.database = database.to_string();
            let (client, handle) = match connect_external(&maintenance_config).await {
                Ok(pair) => pair,
                Err(e) => {
                    last_error = Some(format!("{database}: {e}"));
                    continue;
                }
            };

            let exists = client
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)",
                    &[&config.database],
                )
                .await
                .map_err(|e| {
                    handle.abort();
                    AppError::Internal(format!(
                        "failed to inspect external database '{}': {e}",
                        config.database
                    ))
                })?
                .get::<_, bool>(0);

            if exists {
                diagnostics.push(format!(
                    "External database '{}' already exists but the target connection still failed",
                    config.database
                ));
                handle.abort();
                return Ok(false);
            }

            let sql = Self::create_database_sql(&config.database, &config.username);
            client.batch_execute(&sql).await.map_err(|e| {
                handle.abort();
                AppError::Internal(format!(
                    "failed to create external database '{}': {e}",
                    config.database
                ))
            })?;
            diagnostics.push(format!(
                "Created external database '{}' through maintenance database '{}'",
                config.database, database
            ));
            handle.abort();
            return Ok(true);
        }

        Err(AppError::PostgresUnavailable(format!(
            "external target database is not reachable and maintenance database connection failed: {}",
            last_error.unwrap_or_else(|| "no maintenance database candidate was usable".to_string())
        )))
    }

    fn pg_verify_error(context: &str, error: tokio_postgres::Error) -> AppError {
        AppError::Internal(format!("{context}: {error}"))
    }

    async fn table_content_fingerprint(
        client: &tokio_postgres::Client,
        table: &str,
    ) -> Result<String, AppError> {
        let columns = client
            .query(
                "SELECT column_name
                 FROM information_schema.columns
                 WHERE table_schema = 'public' AND table_name = $1
                 ORDER BY ordinal_position",
                &[&table],
            )
            .await
            .map_err(|e| Self::pg_verify_error("migration content column lookup failed", e))?;
        if columns.is_empty() {
            return Ok("missing".to_string());
        }

        let select_cols = columns
            .iter()
            .map(|row| Self::quote_ident(row.get::<_, &str>("column_name")))
            .collect::<Vec<_>>()
            .join(", ");
        let quoted_table = Self::quote_ident(table);
        let sql = format!(
            "SELECT COALESCE(md5(string_agg(row_hash, E'\\n' ORDER BY row_hash)), md5('')) AS fingerprint
             FROM (
                 SELECT md5(row_to_json(t)::text) AS row_hash
                 FROM (SELECT {select_cols} FROM {quoted_table}) AS t
             ) AS rows"
        );
        client
            .query_one(&sql, &[])
            .await
            .map(|row| row.get::<_, String>("fingerprint"))
            .map_err(|e| Self::pg_verify_error("migration content fingerprint query failed", e))
    }

    async fn compare_migration_content_fingerprints(
        managed: &tokio_postgres::Client,
        external: &tokio_postgres::Client,
    ) -> Result<(), AppError> {
        let mut mismatches = Vec::new();
        for table in MIGRATION_TABLES {
            let managed_fingerprint = Self::table_content_fingerprint(managed, table).await?;
            let external_fingerprint = Self::table_content_fingerprint(external, table).await?;
            if managed_fingerprint != external_fingerprint {
                mismatches.push(format!(
                    "{table}: managed={managed_fingerprint} external={external_fingerprint}"
                ));
            }
        }

        if mismatches.is_empty() {
            Ok(())
        } else {
            Err(AppError::Internal(format!(
                "External migration content fingerprint verification failed: {}",
                mismatches.join("; ")
            )))
        }
    }

    async fn schema_fingerprint(
        client: &tokio_postgres::Client,
        kind: &str,
    ) -> Result<String, AppError> {
        match kind {
            "constraints" => client
                .query_one(
                    "SELECT COALESCE(md5(string_agg(conname || ':' || pg_get_constraintdef(c.oid), E'\n' ORDER BY conrelid::regclass::text, conname)), md5('')) AS fingerprint
                     FROM pg_constraint c
                     WHERE connamespace = 'public'::regnamespace
                       AND conrelid::regclass::text = ANY($1)",
                    &[&MIGRATION_TABLES],
                )
                .await
                .map(|row| row.get::<_, String>("fingerprint"))
                .map_err(|e| {
                    Self::pg_verify_error("migration constraint fingerprint query failed", e)
                }),
            "indexes" => client
                .query_one(
                    "SELECT COALESCE(md5(string_agg(indexname || ':' || indexdef, E'\n' ORDER BY tablename, indexname)), md5('')) AS fingerprint
                     FROM pg_indexes
                     WHERE schemaname = 'public'
                       AND tablename = ANY($1)",
                    &[&MIGRATION_TABLES],
                )
                .await
                .map(|row| row.get::<_, String>("fingerprint"))
                .map_err(|e| Self::pg_verify_error("migration index fingerprint query failed", e)),
            other => Err(AppError::Internal(format!(
                "unknown schema fingerprint kind: {other}"
            ))),
        }
    }

    async fn compare_migration_schema_fingerprints(
        managed: &tokio_postgres::Client,
        external: &tokio_postgres::Client,
    ) -> Result<(), AppError> {
        let mut mismatches = Vec::new();
        for kind in MIGRATION_SCHEMA_KINDS {
            let managed_fingerprint = Self::schema_fingerprint(managed, kind).await?;
            let external_fingerprint = Self::schema_fingerprint(external, kind).await?;
            if managed_fingerprint != external_fingerprint {
                mismatches.push(format!(
                    "{kind}: managed={managed_fingerprint} external={external_fingerprint}"
                ));
            }
        }

        if mismatches.is_empty() {
            Ok(())
        } else {
            Err(AppError::Internal(format!(
                "External migration schema fingerprint verification failed: {}",
                mismatches.join("; ")
            )))
        }
    }

    async fn external_migration_read_write_smoke(
        client: &tokio_postgres::Client,
    ) -> Result<(), AppError> {
        let smoke_key = "__imagedb_external_migration_smoke__";
        client
            .execute(
                "INSERT INTO app_meta (key, value)
             VALUES ($1, '{\"ok\": true}'::jsonb)
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                &[&smoke_key],
            )
            .await
            .map_err(|e| Self::pg_verify_error("external migration smoke write failed", e))?;
        let ok = client
            .query_one(
                "SELECT value->>'ok' = 'true' FROM app_meta WHERE key = $1",
                &[&smoke_key],
            )
            .await
            .map_err(|e| Self::pg_verify_error("external migration smoke read failed", e))?
            .get::<_, bool>(0);
        if !ok {
            return Err(AppError::Internal(
                "external migration read/write smoke query returned unexpected value".to_string(),
            ));
        }
        client
            .execute("DELETE FROM app_meta WHERE key = $1", &[&smoke_key])
            .await
            .map_err(|e| Self::pg_verify_error("external migration smoke cleanup failed", e))?;
        Ok(())
    }

    fn apply_psql_tls_env(command: &mut Command, config: &ConnectionConfig) {
        command.env("PGSSLMODE", config.tls_mode.libpq_sslmode());
        if let Some(path) = &config.ca_cert_path {
            command.env("PGSSLROOTCERT", path);
        }
        if let Some(path) = &config.client_cert_path {
            command.env("PGSSLCERT", path);
        }
        if let Some(path) = &config.client_key_path {
            command.env("PGSSLKEY", path);
        }
    }

    async fn run_command_checked_with_cancel(
        mut command: Command,
        label: &str,
        diagnostics: &mut Vec<String>,
        cancelled: Option<Arc<AtomicBool>>,
        progress_tracker: Option<Arc<Mutex<ExternalMigrationProgress>>>,
    ) -> Result<(), AppError> {
        command.stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| AppError::Internal(format!("failed to launch {label}: {e}")))?;

        loop {
            if Self::migration_cancel_requested(&cancelled) {
                let _ = child.kill().await;
                let msg = format!("external migration cancelled during {label}");
                diagnostics.push(msg.clone());
                Self::set_external_migration_cancelled(
                    &progress_tracker,
                    label,
                    diagnostics.clone(),
                )
                .await;
                return Err(AppError::Internal(msg));
            }

            match child
                .try_wait()
                .map_err(|e| AppError::Internal(format!("failed to wait for {label}: {e}")))?
            {
                Some(status) if status.success() => {
                    diagnostics.push(format!("{label}: OK"));
                    return Ok(());
                }
                Some(_status) => {
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        let _ = pipe.read_to_string(&mut stderr).await;
                    }
                    return Err(AppError::Internal(format!("{label} failed: {stderr}")));
                }
                None => sleep(Duration::from_millis(200)).await,
            }
        }
    }

    fn migration_cancel_requested(cancelled: &Option<Arc<AtomicBool>>) -> bool {
        cancelled
            .as_ref()
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    async fn set_external_migration_progress<F>(
        progress_tracker: &Option<Arc<Mutex<ExternalMigrationProgress>>>,
        update: F,
    ) where
        F: FnOnce(&mut ExternalMigrationProgress),
    {
        if let Some(tracker) = progress_tracker {
            let mut progress = tracker.lock().await;
            update(&mut progress);
        }
    }

    async fn set_external_migration_stage(
        progress_tracker: &Option<Arc<Mutex<ExternalMigrationProgress>>>,
        stage: &str,
        diagnostics: &[String],
    ) {
        Self::set_external_migration_progress(progress_tracker, |progress| {
            progress.state = "running".to_string();
            progress.current_stage = stage.to_string();
            progress.diagnostics = diagnostics.to_vec();
        })
        .await;
    }

    async fn set_external_migration_cancelled(
        progress_tracker: &Option<Arc<Mutex<ExternalMigrationProgress>>>,
        stage: &str,
        diagnostics: Vec<String>,
    ) {
        Self::set_external_migration_progress(progress_tracker, |progress| {
            *progress = ExternalMigrationProgress::cancelled(stage, diagnostics);
        })
        .await;
    }

    async fn finish_external_migration_progress(
        progress_tracker: &Option<Arc<Mutex<ExternalMigrationProgress>>>,
        result: &ExternalMigrationResult,
    ) {
        Self::set_external_migration_progress(progress_tracker, |progress| {
            *progress = ExternalMigrationProgress::completed(result);
        })
        .await;
    }

    async fn check_external_migration_cancelled(
        cancelled: &Option<Arc<AtomicBool>>,
        progress_tracker: &Option<Arc<Mutex<ExternalMigrationProgress>>>,
        stage: &str,
        diagnostics: &[String],
    ) -> Result<(), AppError> {
        if Self::migration_cancel_requested(cancelled) {
            Self::set_external_migration_cancelled(progress_tracker, stage, diagnostics.to_vec())
                .await;
            return Err(AppError::Internal(format!(
                "external migration cancelled during {stage}; profile not switched"
            )));
        }
        Ok(())
    }

    fn external_unreachable_diagnostics(error: impl Into<String>) -> Vec<String> {
        vec![
            error.into(),
            "Active external PostgreSQL profile is unreachable; use the controlled switch-to-managed action to fall back without modifying external data".to_string(),
        ]
    }

    pub async fn get_state(&self) -> DatabaseState {
        let mut mgr = self.postgres_manager.lock().await;
        let settings = self.settings.lock().await;
        let mode = settings
            .get()
            .database_mode
            .as_deref()
            .and_then(DatabaseMode::from_str_opt);

        let (status, pgvector, migration_version, diagnostics) =
            if matches!(mode, Some(DatabaseMode::External)) {
                let external_config = Self::connection_config_from_settings(
                    settings.get(),
                    self.credentials.as_ref(),
                );
                match external_config {
                    Ok(Some(config)) => match connect_external(&config).await {
                        Ok((client, handle)) => {
                            let pgvector = client
                            .query_one(
                                "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='vector')",
                                &[],
                            )
                            .await
                            .map(|row| row.get::<_, bool>(0))
                            .unwrap_or(false);
                            let migration_version = MigrationRunner::current_version(&client)
                                .await
                                .ok()
                                .flatten();
                            handle.abort();
                            mgr.use_external_profile(config);
                            (
                                DatabaseStatus::Connected,
                                pgvector,
                                migration_version,
                                vec!["Active external PostgreSQL profile is reachable".to_string()],
                            )
                        }
                        Err(e) => (
                            DatabaseStatus::Error(e.to_string()),
                            false,
                            None,
                            Self::external_unreachable_diagnostics(e.to_string()),
                        ),
                    },
                    Ok(None) => (
                        DatabaseStatus::Error(
                            "External database profile is incomplete".to_string(),
                        ),
                        false,
                        None,
                        vec!["External database profile is incomplete".to_string()],
                    ),
                    Err(e) => (
                        DatabaseStatus::Error(e.to_string()),
                        false,
                        None,
                        vec![e.to_string()],
                    ),
                }
            } else if mgr.is_server_running() && mgr.binaries_available() {
                match mgr.connect().await {
                    Ok((client, handle)) => {
                        let pgvector = client
                            .query_one(
                                "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='vector')",
                                &[],
                            )
                            .await
                            .map(|row| row.get::<_, bool>(0))
                            .unwrap_or(false);

                        let migration_version = MigrationRunner::current_version(&client)
                            .await
                            .ok()
                            .flatten();

                        handle.abort();
                        (
                            DatabaseStatus::Connected,
                            pgvector,
                            migration_version,
                            vec![],
                        )
                    }
                    Err(e) => (
                        DatabaseStatus::Error(e.to_string()),
                        false,
                        None,
                        vec![e.to_string()],
                    ),
                }
            } else if !mgr.binaries_available() {
                (
                    DatabaseStatus::BinariesMissing(
                        "安装包不完整：缺少内置 PostgreSQL 运行文件，请重新安装 ImageDB。"
                            .to_string(),
                    ),
                    false,
                    None,
                    mgr.diagnostics().to_vec(),
                )
            } else if mgr.cluster_files_exist() {
                // An initialized cluster exists on disk (PG_VERSION, base/,
                // global/, pg_wal/) but the server is not running in this
                // process — i.e. the app was restarted. Bring the managed
                // instance back up instead of reporting NotInitialized
                // (which would block the dashboard and every scan/commit).
                match mgr.initialize().await {
                    Ok(probe) if probe.connection_ok => match mgr.connect().await {
                        Ok((client, handle)) => {
                            let pgvector = client
                            .query_one(
                                "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='vector')",
                                &[],
                            )
                            .await
                            .map(|row| row.get::<_, bool>(0))
                            .unwrap_or(false);
                            let migration_version = MigrationRunner::current_version(&client)
                                .await
                                .ok()
                                .flatten();
                            handle.abort();
                            (
                                DatabaseStatus::Connected,
                                pgvector,
                                migration_version,
                                vec!["Managed PostgreSQL restarted on launch".to_string()],
                            )
                        }
                        Err(e) => (
                            DatabaseStatus::Error(e.to_string()),
                            false,
                            None,
                            vec![e.to_string()],
                        ),
                    },
                    Ok(probe) => {
                        // initialize() could not reach the server (e.g.
                        // pg_ctl start failed). Surface the diagnostics so
                        // the user can act, but keep the cluster on disk.
                        let mut diags = probe.diagnostics.clone();
                        if diags.is_empty() {
                            diags = mgr.diagnostics().to_vec();
                        }
                        (
                            DatabaseStatus::Error("Managed PostgreSQL failed to start".to_string()),
                            false,
                            None,
                            diags,
                        )
                    }
                    Err(e) => (
                        DatabaseStatus::Error(e.to_string()),
                        false,
                        None,
                        vec![e.to_string()],
                    ),
                }
            } else {
                (DatabaseStatus::NotInitialized, false, None, vec![])
            };

        let managed_config = if mgr.binaries_available() {
            Some(ManagedDbConfig {
                data_dir: mgr.data_dir().display().to_string(),
                port: mgr.port(),
                username: mgr.username().to_string(),
                database: mgr.database().to_string(),
            })
        } else {
            None
        };

        let external_config = {
            let s = settings.get();
            Self::connection_config_from_settings_without_secret(s)
        };

        DatabaseState {
            mode,
            status,
            managed_config,
            external_config,
            pgvector_available: pgvector,
            migration_version,
            diagnostics,
        }
    }

    pub async fn initialize_managed(&self) -> Result<DatabaseState, AppError> {
        let mut mgr = self.postgres_manager.lock().await;
        mgr.use_managed_profile();

        if !mgr.binaries_available() {
            return Ok(DatabaseState {
                mode: Some(DatabaseMode::ManagedLocal),
                status: DatabaseStatus::BinariesMissing(
                    "安装包不完整：缺少内置 PostgreSQL 运行文件，请重新安装 ImageDB。".to_string(),
                ),
                managed_config: None,
                external_config: None,
                pgvector_available: false,
                migration_version: None,
                diagnostics: mgr.diagnostics().to_vec(),
            });
        }

        let probe_result = mgr.initialize().await?;

        if !probe_result.connection_ok {
            return Ok(DatabaseState {
                mode: Some(DatabaseMode::ManagedLocal),
                status: DatabaseStatus::Error(
                    "Database initialization failed - connection test failed".to_string(),
                ),
                managed_config: Some(ManagedDbConfig {
                    data_dir: probe_result.data_dir.unwrap_or_default(),
                    port: probe_result.port.unwrap_or(0),
                    username: mgr.username().to_string(),
                    database: mgr.database().to_string(),
                }),
                external_config: None,
                pgvector_available: false,
                migration_version: None,
                diagnostics: probe_result.diagnostics,
            });
        }

        let (mut client, handle) = mgr.connect().await?;
        let applied = MigrationRunner::run_pending(&mut client).await?;
        let version = MigrationRunner::current_version(&client).await?;
        handle.abort();

        let mut settings = self.settings.lock().await;
        settings.set_database_mode("managed_local")?;
        settings.set_first_run_completed(true)?;

        Ok(DatabaseState {
            mode: Some(DatabaseMode::ManagedLocal),
            status: DatabaseStatus::Connected,
            managed_config: Some(ManagedDbConfig {
                data_dir: probe_result.data_dir.unwrap_or_default(),
                port: probe_result.port.unwrap_or(0),
                username: mgr.username().to_string(),
                database: mgr.database().to_string(),
            }),
            external_config: None,
            pgvector_available: probe_result.pgvector_available,
            migration_version: version,
            diagnostics: {
                let mut d = probe_result.diagnostics;
                if !applied.is_empty() {
                    d.push(format!("Applied migrations: {}", applied.join(", ")));
                }
                d
            },
        })
    }

    pub async fn switch_to_managed(&self) -> Result<DatabaseState, AppError> {
        self.initialize_managed().await
    }

    pub async fn test_external_connection(
        &self,
        config: &ConnectionConfig,
    ) -> Result<ExternalCheckResult, AppError> {
        let config = self.with_stored_password(config)?;
        let mut diagnostics = Vec::new();
        let mut checks = Vec::new();

        if matches!(config.tls_mode, TlsMode::Disable) {
            checks.push(ExternalPreflightCheck::warn(
                "tls.mode",
                "TLS disabled; only use this for trusted local or isolated network testing",
            ));
        } else {
            checks.push(ExternalPreflightCheck::pass(
                "tls.mode",
                format!("TLS mode: {}", config.tls_mode.as_str()),
            ));
        }

        match (&config.client_cert_path, &config.client_key_path) {
            (Some(_), Some(_)) => {
                checks.push(ExternalPreflightCheck::pass(
                    "tls.client_certificate",
                    "Client certificate and private key paths are configured for mutual TLS",
                ));
            }
            (Some(_), None) | (None, Some(_)) => {
                checks.push(ExternalPreflightCheck::fail(
                    "tls.client_certificate",
                    "Client certificate and private key must be configured together",
                ));
            }
            (None, None) => {}
        }

        let (client, handle) = match connect_external(&config).await {
            Ok(pair) => {
                diagnostics.push("External connection successful".to_string());
                checks.push(ExternalPreflightCheck::pass(
                    "connection",
                    "Connected to external PostgreSQL",
                ));
                pair
            }
            Err(e) => {
                diagnostics.push(format!("Connection failed: {e}"));
                checks.push(ExternalPreflightCheck::fail(
                    "connection",
                    format!("Connection failed: {e}"),
                ));
                return Ok(ExternalCheckResult {
                    connection_ok: false,
                    version: None,
                    version_ok: false,
                    tls_mode: config.tls_mode.clone(),
                    tls_ok: false,
                    pgvector_available: false,
                    can_create_extension: false,
                    can_create_tables: false,
                    can_modify_schema: false,
                    read_write_ok: false,
                    encoding_ok: false,
                    timezone_ok: false,
                    not_read_only: false,
                    migration_state_ok: false,
                    schema_compatible: false,
                    migration_version: None,
                    checks,
                    diagnostics,
                });
            }
        };

        let query_timeout = Duration::from_secs(config.query_timeout_secs.max(1));

        let version = timeout(query_timeout, client.query_one("SELECT version()", &[]))
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|row| row.try_get::<_, String>(0).ok());

        let (version_ok, version_diagnostic) = match version.as_deref() {
            Some(v) => match parse_postgres_major(v) {
                Some(n) if n >= 14 => (true, format!("PostgreSQL version: {v} (major {n})")),
                Some(n) => (
                    false,
                    format!(
                        "PostgreSQL version {v} is below the minimum supported major version 14 \
                         (detected major {n})"
                    ),
                ),
                None => (
                    false,
                    format!(
                        "Could not parse PostgreSQL major version from: {v} \
                         (expected a substring like 'PostgreSQL <number>')"
                    ),
                ),
            },
            None => (
                false,
                "SELECT version() returned no usable string".to_string(),
            ),
        };

        diagnostics.push(version_diagnostic);
        checks.push(if version_ok {
            ExternalPreflightCheck::pass("postgres.version", "PostgreSQL version is supported")
        } else {
            ExternalPreflightCheck::fail(
                "postgres.version",
                "PostgreSQL major version is unsupported or unknown",
            )
        });

        let pgvector_available = timeout(
            query_timeout,
            client.query_one(
                "SELECT EXISTS(SELECT 1 FROM pg_available_extensions WHERE name='vector')",
                &[],
            ),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .map(|row| row.get::<_, bool>(0))
        .unwrap_or(false);

        let pgvector_installed = timeout(
            query_timeout,
            client.query_one(
                "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='vector')",
                &[],
            ),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .map(|row| row.get::<_, bool>(0))
        .unwrap_or(false);

        let can_create_extension = if pgvector_installed {
            true
        } else if pgvector_available {
            timeout(
                query_timeout,
                client.batch_execute(
                    "BEGIN;
                     CREATE EXTENSION IF NOT EXISTS vector;
                     ROLLBACK;",
                ),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .is_some()
        } else {
            false
        };

        if pgvector_installed {
            diagnostics.push("pgvector extension installed".to_string());
            checks.push(ExternalPreflightCheck::pass(
                "pgvector.installed",
                "vector extension is installed in the target database",
            ));
        } else if pgvector_available && can_create_extension {
            diagnostics.push("pgvector extension available and creatable".to_string());
            checks.push(ExternalPreflightCheck::pass(
                "pgvector.create",
                "vector extension is available and current role can create it",
            ));
        } else if pgvector_available {
            diagnostics.push("pgvector extension available but not creatable".to_string());
            checks.push(ExternalPreflightCheck::fail(
                "pgvector.create",
                "vector extension is available but current role cannot create it",
            ));
        } else {
            diagnostics.push("pgvector extension is not available".to_string());
            checks.push(ExternalPreflightCheck::fail(
                "pgvector.available",
                "vector extension is not installed on this PostgreSQL server",
            ));
        }

        let not_read_only = timeout(
            query_timeout,
            client.query_one(
                "SELECT current_setting('transaction_read_only') = 'off'",
                &[],
            ),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .map(|row| row.get::<_, bool>(0))
        .unwrap_or(false);
        checks.push(if not_read_only {
            ExternalPreflightCheck::pass("database.read_write", "Database accepts write sessions")
        } else {
            ExternalPreflightCheck::fail(
                "database.read_write",
                "Connection is read-only or points at a read replica",
            )
        });

        let encoding_ok = timeout(
            query_timeout,
            client.query_one("SELECT current_setting('server_encoding')", &[]),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(|row| row.try_get::<_, String>(0).ok())
        .map(|encoding| {
            encoding.eq_ignore_ascii_case("UTF8") || encoding.eq_ignore_ascii_case("UTF-8")
        })
        .unwrap_or(false);
        checks.push(if encoding_ok {
            ExternalPreflightCheck::pass("database.encoding", "Database encoding is UTF-8")
        } else {
            ExternalPreflightCheck::fail("database.encoding", "Database encoding is not UTF-8")
        });

        let timezone_ok = timeout(query_timeout, client.query_one("SELECT now()", &[]))
            .await
            .ok()
            .and_then(Result::ok)
            .is_some();
        checks.push(if timezone_ok {
            ExternalPreflightCheck::pass("database.time", "Database time functions are usable")
        } else {
            ExternalPreflightCheck::fail("database.time", "Database time function check failed")
        });

        let can_create_tables = timeout(
            query_timeout,
            client.batch_execute(
                "CREATE TABLE IF NOT EXISTS _imagedb_permission_test (id int);
                 DROP TABLE IF EXISTS _imagedb_permission_test;",
            ),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .is_some();

        if can_create_tables {
            diagnostics.push("Table creation permission: OK".to_string());
            checks.push(ExternalPreflightCheck::pass(
                "permission.tables",
                "Current role can create and drop tables",
            ));
        } else {
            diagnostics.push("Table creation permission: FAILED".to_string());
            checks.push(ExternalPreflightCheck::fail(
                "permission.tables",
                "Current role cannot create required application tables",
            ));
        }

        let can_modify_schema = timeout(
            query_timeout,
            client.batch_execute(
                "CREATE SCHEMA IF NOT EXISTS _imagedb_preflight;
                 DROP SCHEMA IF EXISTS _imagedb_preflight;",
            ),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .is_some();
        checks.push(if can_modify_schema {
            ExternalPreflightCheck::pass(
                "permission.schema",
                "Current role can create and drop schemas",
            )
        } else {
            ExternalPreflightCheck::fail("permission.schema", "Current role cannot modify schemas")
        });

        let schema_migrations_exists = timeout(
            query_timeout,
            client.query_one(
                "SELECT to_regclass('public.schema_migrations') IS NOT NULL",
                &[],
            ),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(|row| {
            let exists = row.get::<_, bool>(0);
            exists.then_some(())
        });

        let (migration_version, migration_history_ok, migration_history_diagnostic) =
            if schema_migrations_exists.is_some() {
                match MigrationRunner::get_applied_migrations(&client).await {
                    Ok(applied) => {
                        let current = applied.last().cloned();
                        match MigrationRunner::validate_applied_versions(&applied) {
                            Ok(()) => (
                                current,
                                true,
                                format!(
                                    "ImageDB migration history is compatible; current version: {}",
                                    applied.last().map(String::as_str).unwrap_or("<empty>")
                                ),
                            ),
                            Err(e) => (current, false, e),
                        }
                    }
                    Err(e) => (
                        None,
                        false,
                        format!("failed to inspect ImageDB migration history: {e}"),
                    ),
                }
            } else {
                (
                    None,
                    true,
                    "No ImageDB migration table exists; target is empty or unmanaged".to_string(),
                )
            };
        diagnostics.push(migration_history_diagnostic.clone());

        let image_tables_empty_or_versioned = timeout(
            query_timeout,
            client.query_one(
                "SELECT NOT EXISTS (
                    SELECT 1
                    FROM information_schema.tables
                    WHERE table_schema = 'public'
                      AND table_name IN (
                        'library_roots',
                        'import_runs',
                        'source_images',
                        'library_albums',
                        'library_images',
                        'file_transactions'
                      )
                )
                OR to_regclass('public.schema_migrations') IS NOT NULL",
                &[],
            ),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .map(|row| row.get::<_, bool>(0))
        .unwrap_or(false);

        let migration_state_ok = migration_history_ok && image_tables_empty_or_versioned;
        let schema_compatible = migration_state_ok;
        checks.push(if migration_history_ok {
            ExternalPreflightCheck::pass(
                "migration.history",
                "ImageDB migration history is known and upgradeable",
            )
        } else {
            ExternalPreflightCheck::fail("migration.history", migration_history_diagnostic.clone())
        });
        checks.push(if image_tables_empty_or_versioned {
            ExternalPreflightCheck::pass(
                "migration.state",
                "Migration table state is empty or compatible",
            )
        } else {
            ExternalPreflightCheck::fail(
                "migration.state",
                "ImageDB-like tables exist without a schema_migrations table",
            )
        });

        handle.abort();

        Ok(ExternalCheckResult {
            connection_ok: true,
            version,
            version_ok,
            tls_mode: config.tls_mode.clone(),
            tls_ok: !matches!(config.tls_mode, TlsMode::Disable),
            pgvector_available: pgvector_installed || pgvector_available,
            can_create_extension,
            can_create_tables,
            can_modify_schema,
            read_write_ok: not_read_only && can_create_tables,
            encoding_ok,
            timezone_ok,
            not_read_only,
            migration_state_ok,
            schema_compatible,
            migration_version,
            checks,
            diagnostics,
        })
    }

    pub async fn initialize_external(
        &self,
        config: &ConnectionConfig,
    ) -> Result<DatabaseState, AppError> {
        let effective_config = self.with_stored_password(config)?;
        let mut diagnostics = Vec::new();
        Self::ensure_external_database_exists(&effective_config, &mut diagnostics).await?;
        let check = self.test_external_connection(&effective_config).await?;
        diagnostics.extend(check.diagnostics.clone());

        if !check.connection_ok
            || !check.version_ok
            || !check.read_write_ok
            || !check.encoding_ok
            || !check.timezone_ok
            || !check.migration_state_ok
            || !check.schema_compatible
            || !check.pgvector_available
        {
            return Ok(DatabaseState {
                mode: Some(DatabaseMode::External),
                status: DatabaseStatus::Error(
                    "External database preflight failed; active profile was not switched"
                        .to_string(),
                ),
                managed_config: None,
                external_config: Some(Self::redacted_config(&effective_config)),
                pgvector_available: false,
                migration_version: None,
                diagnostics,
            });
        }

        let (mut client, handle) = connect_external(&effective_config).await?;

        client
            .batch_execute("CREATE EXTENSION IF NOT EXISTS vector")
            .await
            .map_err(|e| {
                AppError::Internal(format!(
                    "failed to create vector extension on external database: {e}"
                ))
            })?;
        let applied = MigrationRunner::run_pending(&mut client).await?;
        let version = MigrationRunner::current_version(&client).await?;
        handle.abort();

        self.store_and_activate_external_profile(&effective_config)
            .await?;

        Ok(DatabaseState {
            mode: Some(DatabaseMode::External),
            status: DatabaseStatus::Connected,
            managed_config: None,
            external_config: Some(Self::redacted_config(&effective_config)),
            pgvector_available: check.pgvector_available,
            migration_version: version,
            diagnostics: {
                let mut d = diagnostics;
                if !applied.is_empty() {
                    d.push(format!("Applied migrations: {}", applied.join(", ")));
                }
                d
            },
        })
    }

    pub async fn migrate_managed_to_external(
        &self,
        config: &ConnectionConfig,
    ) -> Result<ExternalMigrationResult, AppError> {
        self.migrate_managed_to_external_inner(config, None, None)
            .await
    }

    pub async fn migrate_managed_to_external_with_control(
        &self,
        config: &ConnectionConfig,
        cancelled: Arc<AtomicBool>,
        progress_tracker: Arc<Mutex<ExternalMigrationProgress>>,
    ) -> Result<ExternalMigrationResult, AppError> {
        self.migrate_managed_to_external_inner(config, Some(cancelled), Some(progress_tracker))
            .await
    }

    async fn migrate_managed_to_external_inner(
        &self,
        config: &ConnectionConfig,
        cancelled: Option<Arc<AtomicBool>>,
        progress_tracker: Option<Arc<Mutex<ExternalMigrationProgress>>>,
    ) -> Result<ExternalMigrationResult, AppError> {
        let effective_config = self.with_stored_password(config)?;
        let mut diagnostics = Vec::new();

        Self::set_external_migration_stage(&progress_tracker, "preflight", &diagnostics).await;
        Self::check_external_migration_cancelled(
            &cancelled,
            &progress_tracker,
            "preflight",
            &diagnostics,
        )
        .await?;
        Self::ensure_external_database_exists(&effective_config, &mut diagnostics).await?;
        Self::set_external_migration_stage(&progress_tracker, "preflight", &diagnostics).await;
        let check = self.test_external_connection(&effective_config).await?;
        diagnostics.extend(check.diagnostics.clone());
        if !check.connection_ok
            || !check.version_ok
            || !check.read_write_ok
            || !check.encoding_ok
            || !check.timezone_ok
            || !check.migration_state_ok
            || !check.schema_compatible
            || !check.pgvector_available
        {
            let result = ExternalMigrationResult {
                switched: false,
                backup_path: None,
                migration_version: None,
                row_counts: Vec::new(),
                diagnostics,
            };
            Self::finish_external_migration_progress(&progress_tracker, &result).await;
            return Ok(result);
        }

        Self::set_external_migration_stage(&progress_tracker, "managed_source", &diagnostics).await;
        Self::check_external_migration_cancelled(
            &cancelled,
            &progress_tracker,
            "managed_source",
            &diagnostics,
        )
        .await?;
        let (
            managed_client,
            managed_handle,
            pg_dump,
            psql,
            backup_dir,
            source_port,
            source_user,
            source_db,
            source_password,
        ) = {
            let mut mgr = self.postgres_manager.lock().await;
            mgr.use_managed_profile();
            if !mgr.is_server_running() {
                let probe = mgr.initialize().await?;
                if !probe.connection_ok {
                    let result = ExternalMigrationResult {
                        switched: false,
                        backup_path: None,
                        migration_version: None,
                        row_counts: Vec::new(),
                        diagnostics: probe.diagnostics,
                    };
                    Self::finish_external_migration_progress(&progress_tracker, &result).await;
                    return Ok(result);
                }
                diagnostics.extend(probe.diagnostics);
            } else {
                diagnostics.push("Managed PostgreSQL source is already running".to_string());
            }

            let pg_dump = mgr
                .pg_dump_path()
                .ok_or_else(|| {
                    AppError::PostgresUnavailable(
                        "pg_dump binary is required for managed-to-external migration".to_string(),
                    )
                })?
                .to_path_buf();
            let psql = mgr
                .psql_path()
                .ok_or_else(|| {
                    AppError::PostgresUnavailable(
                        "psql binary is required for managed-to-external migration".to_string(),
                    )
                })?
                .to_path_buf();
            let backup_dir = mgr
                .app_data_dir()
                .ok_or_else(|| {
                    AppError::Internal("managed database data dir has no parent".to_string())
                })?
                .join("postgres_backups")
                .join("external_migrations");
            let source_port = mgr.port();
            let source_user = mgr.username().to_string();
            let source_db = mgr.database().to_string();
            let source_password = mgr.password().map(ToOwned::to_owned);
            let (client, handle) = mgr.connect().await?;
            (
                client,
                handle,
                pg_dump,
                psql,
                backup_dir,
                source_port,
                source_user,
                source_db,
                source_password,
            )
        };

        Self::set_external_migration_stage(&progress_tracker, "external_target", &diagnostics)
            .await;
        if let Err(e) = Self::check_external_migration_cancelled(
            &cancelled,
            &progress_tracker,
            "external_target",
            &diagnostics,
        )
        .await
        {
            managed_handle.abort();
            return Err(e);
        }
        let (mut external_client, external_handle) = connect_external(&effective_config).await?;
        if Self::external_target_has_rows(&external_client).await? {
            external_handle.abort();
            managed_handle.abort();
            diagnostics.push(
                "External target already contains ImageDB rows; migration refused before switch"
                    .to_string(),
            );
            let result = ExternalMigrationResult {
                switched: false,
                backup_path: None,
                migration_version: None,
                row_counts: Vec::new(),
                diagnostics,
            };
            Self::finish_external_migration_progress(&progress_tracker, &result).await;
            return Ok(result);
        }

        external_client
            .batch_execute("CREATE EXTENSION IF NOT EXISTS vector")
            .await
            .map_err(|e| {
                AppError::Internal(format!(
                    "failed to create vector extension on external database: {e}"
                ))
            })?;
        MigrationRunner::run_pending(&mut external_client).await?;
        external_handle.abort();

        Self::set_external_migration_stage(&progress_tracker, "backup", &diagnostics).await;
        if let Err(e) = Self::check_external_migration_cancelled(
            &cancelled,
            &progress_tracker,
            "backup",
            &diagnostics,
        )
        .await
        {
            managed_handle.abort();
            return Err(e);
        }
        tokio::fs::create_dir_all(&backup_dir).await?;
        let stamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let backup_path = backup_dir.join(format!("managed-to-external-{stamp}.sql"));
        let tmp_backup_path = backup_path.with_extension("sql.tmp");

        let mut dump = Command::new(&pg_dump);
        dump.arg("--data-only")
            .arg("--no-owner")
            .arg("--no-privileges")
            .arg("--exclude-table-data=schema_migrations")
            .arg("--host=127.0.0.1")
            .arg(format!("--port={source_port}"))
            .arg(format!("--username={source_user}"))
            .arg(format!("--dbname={source_db}"))
            .arg("--file")
            .arg(&tmp_backup_path);
        if let Some(password) = &source_password {
            dump.env("PGPASSWORD", password);
        }
        if let Err(e) = Self::run_command_checked_with_cancel(
            dump,
            "managed pg_dump",
            &mut diagnostics,
            cancelled.clone(),
            progress_tracker.clone(),
        )
        .await
        {
            let _ = tokio::fs::remove_file(&tmp_backup_path).await;
            managed_handle.abort();
            return Err(e);
        }
        if let Err(e) = tokio::fs::rename(&tmp_backup_path, &backup_path).await {
            managed_handle.abort();
            return Err(e.into());
        }
        Self::set_external_migration_progress(&progress_tracker, |progress| {
            progress.backup_path = Some(backup_path.display().to_string());
            progress.diagnostics = diagnostics.clone();
        })
        .await;

        Self::set_external_migration_stage(&progress_tracker, "import", &diagnostics).await;
        if let Err(e) = Self::check_external_migration_cancelled(
            &cancelled,
            &progress_tracker,
            "import",
            &diagnostics,
        )
        .await
        {
            managed_handle.abort();
            return Err(e);
        }
        let mut import = Command::new(&psql);
        import
            .arg("--set=ON_ERROR_STOP=1")
            .arg(format!("--host={}", effective_config.host))
            .arg(format!("--port={}", effective_config.port))
            .arg(format!("--username={}", effective_config.username))
            .arg(format!("--dbname={}", effective_config.database))
            .arg("--file")
            .arg(&backup_path);
        if let Some(password) = &effective_config.password {
            import.env("PGPASSWORD", password);
        }
        Self::apply_psql_tls_env(&mut import, &effective_config);
        if let Err(e) = Self::run_command_checked_with_cancel(
            import,
            "external psql import",
            &mut diagnostics,
            cancelled.clone(),
            progress_tracker.clone(),
        )
        .await
        {
            managed_handle.abort();
            return Err(e);
        }

        Self::set_external_migration_stage(&progress_tracker, "verify", &diagnostics).await;
        if let Err(e) = Self::check_external_migration_cancelled(
            &cancelled,
            &progress_tracker,
            "verify",
            &diagnostics,
        )
        .await
        {
            managed_handle.abort();
            return Err(e);
        }
        let (external_client, external_handle) = connect_external(&effective_config).await?;
        let row_counts =
            Self::compare_migration_row_counts(&managed_client, &external_client).await?;
        let all_counts_match = row_counts.iter().all(|count| count.matches);
        let content_fingerprints_match =
            Self::compare_migration_content_fingerprints(&managed_client, &external_client).await;
        let schema_fingerprints_match =
            Self::compare_migration_schema_fingerprints(&managed_client, &external_client).await;
        let read_write_smoke = Self::external_migration_read_write_smoke(&external_client).await;
        let migration_version = MigrationRunner::current_version(&external_client).await?;
        external_handle.abort();
        managed_handle.abort();

        if !all_counts_match {
            diagnostics.push(
                "External migration row count verification failed; profile not switched"
                    .to_string(),
            );
            let result = ExternalMigrationResult {
                switched: false,
                backup_path: Some(backup_path.display().to_string()),
                migration_version,
                row_counts,
                diagnostics,
            };
            Self::finish_external_migration_progress(&progress_tracker, &result).await;
            return Ok(result);
        }
        if let Err(e) = content_fingerprints_match {
            diagnostics.push(e.to_string());
            let result = ExternalMigrationResult {
                switched: false,
                backup_path: Some(backup_path.display().to_string()),
                migration_version,
                row_counts,
                diagnostics,
            };
            Self::finish_external_migration_progress(&progress_tracker, &result).await;
            return Ok(result);
        }
        diagnostics.push("External migration table content fingerprints verified".to_string());

        if let Err(e) = schema_fingerprints_match {
            diagnostics.push(e.to_string());
            let result = ExternalMigrationResult {
                switched: false,
                backup_path: Some(backup_path.display().to_string()),
                migration_version,
                row_counts,
                diagnostics,
            };
            Self::finish_external_migration_progress(&progress_tracker, &result).await;
            return Ok(result);
        }
        diagnostics.push("External migration constraints and indexes verified".to_string());

        if let Err(e) = read_write_smoke {
            diagnostics.push(e.to_string());
            let result = ExternalMigrationResult {
                switched: false,
                backup_path: Some(backup_path.display().to_string()),
                migration_version,
                row_counts,
                diagnostics,
            };
            Self::finish_external_migration_progress(&progress_tracker, &result).await;
            return Ok(result);
        }
        diagnostics.push("External migration read/write smoke verified".to_string());

        Self::set_external_migration_stage(&progress_tracker, "switch", &diagnostics).await;
        Self::check_external_migration_cancelled(
            &cancelled,
            &progress_tracker,
            "switch",
            &diagnostics,
        )
        .await?;
        self.store_and_activate_external_profile(&effective_config)
            .await?;
        diagnostics.push("External database verified and activated".to_string());

        let result = ExternalMigrationResult {
            switched: true,
            backup_path: Some(backup_path.display().to_string()),
            migration_version,
            row_counts,
            diagnostics,
        };
        Self::finish_external_migration_progress(&progress_tracker, &result).await;
        Ok(result)
    }
}

fn parse_postgres_major(version: &str) -> Option<u32> {
    let marker = "PostgreSQL ";
    let idx = version.find(marker)?;
    let after = &version[idx + marker.len()..];
    let digits_end = after
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after.len());
    if digits_end == 0 {
        return None;
    }
    after[..digits_end].parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_postgres_major_standard() {
        let v = "PostgreSQL 16.4 on x86_64-pc-linux-gnu, compiled by gcc 11.4.0, 64-bit";
        assert_eq!(parse_postgres_major(v), Some(16));
    }

    #[test]
    fn parse_postgres_major_future() {
        let v = "PostgreSQL 18.0 on x86_64-pc-linux-gnu";
        assert_eq!(parse_postgres_major(v), Some(18));
    }

    #[test]
    fn parse_postgres_major_dev() {
        let v = "PostgreSQL 19devel on x86_64";
        assert_eq!(parse_postgres_major(v), Some(19));
    }

    #[test]
    fn parse_postgres_major_old() {
        let v = "PostgreSQL 13.14 on x86_64";
        assert_eq!(parse_postgres_major(v), Some(13));
    }

    #[test]
    fn parse_postgres_major_malformed() {
        assert_eq!(parse_postgres_major("MySQL 8.0"), None);
        assert_eq!(parse_postgres_major("PostgreSQL abc"), None);
        assert_eq!(parse_postgres_major(""), None);
    }

    #[tokio::test]
    async fn migrate_managed_to_external_cancelled_before_preflight_never_switches() {
        use crate::infrastructure::postgres::PostgresManager;
        use crate::infrastructure::secrets::CredentialStore;
        use crate::infrastructure::settings::SettingsStore;
        use tempfile::TempDir;

        let app_tmp = TempDir::new().unwrap();
        let settings_tmp = TempDir::new().unwrap();
        let manager = Arc::new(Mutex::new(PostgresManager::new(app_tmp.path())));
        let settings = Arc::new(Mutex::new(SettingsStore::new(settings_tmp.path()).unwrap()));
        let credentials =
            Arc::new(CredentialStore::new_file_for_tests(settings_tmp.path()).unwrap());
        let service = DatabaseService::new(manager, settings.clone(), credentials);
        let cancelled = Arc::new(AtomicBool::new(true));
        let progress = Arc::new(Mutex::new(ExternalMigrationProgress::idle()));
        let config = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: 5432,
            database: "imagedb".to_string(),
            username: "imagedb".to_string(),
            password: Some("secret".to_string()),
            tls_mode: TlsMode::Disable,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            connect_timeout_secs: 1,
            query_timeout_secs: 1,
            profile_name: Some("cancel-test".to_string()),
        };

        let err = service
            .migrate_managed_to_external_with_control(&config, cancelled, progress.clone())
            .await
            .expect_err("cancelled migration should return an error");
        assert!(err.to_string().contains("cancelled"));

        let p = progress.lock().await;
        assert_eq!(p.state, "cancelled");
        assert_eq!(p.current_stage, "preflight");
        assert!(p.cancel_requested);
        assert!(!p.switched);
        drop(p);

        let stored = settings.lock().await;
        assert_ne!(stored.get().database_mode.as_deref(), Some("external"));
    }

    #[tokio::test]
    async fn cancellable_migration_command_kills_running_child_and_marks_progress() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(ExternalMigrationProgress::running("import")));
        let cancelled_for_task = cancelled.clone();
        let progress_for_task = progress.clone();

        let task = tokio::spawn(async move {
            let mut diagnostics = Vec::new();
            let command = long_running_command();
            let result = DatabaseService::run_command_checked_with_cancel(
                command,
                "test long-running import",
                &mut diagnostics,
                Some(cancelled_for_task),
                Some(progress_for_task),
            )
            .await;
            (result, diagnostics)
        });

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        cancelled.store(true, Ordering::Relaxed);

        let (result, diagnostics) = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("cancelled child command should finish promptly")
            .expect("task should not panic");

        let err = result.expect_err("cancelled command should fail");
        assert!(err
            .to_string()
            .contains("external migration cancelled during test long-running import"));
        assert!(diagnostics
            .iter()
            .any(|d| d.contains("external migration cancelled during test long-running import")));

        let progress = progress.lock().await;
        assert_eq!(progress.state, "cancelled");
        assert_eq!(progress.current_stage, "test long-running import");
        assert!(progress.cancel_requested);
        assert!(!progress.switched);
    }

    #[test]
    fn external_unreachable_diagnostics_points_to_controlled_managed_fallback() {
        let diagnostics = DatabaseService::external_unreachable_diagnostics(
            "external TLS connection failed: connection refused",
        );
        assert_eq!(
            diagnostics.first().map(String::as_str),
            Some("external TLS connection failed: connection refused")
        );
        assert!(diagnostics.iter().any(|d| {
            d.contains("controlled switch-to-managed action")
                && d.contains("without modifying external data")
        }));
    }

    #[test]
    fn create_database_sql_quotes_database_and_owner_identifiers() {
        let sql = DatabaseService::create_database_sql("image db", "helw\"admin");
        assert_eq!(
            sql,
            "CREATE DATABASE \"image db\" OWNER \"helw\"\"admin\" ENCODING 'UTF8' TEMPLATE template0"
        );
    }

    #[cfg(windows)]
    fn long_running_command() -> Command {
        let mut command = Command::new("powershell");
        command.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
        command
    }

    #[cfg(not(windows))]
    fn long_running_command() -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        command
    }

    #[cfg(feature = "real-db-tests")]
    #[tokio::test]
    #[ignore]
    async fn real_external_empty_database_preflights_and_initializes_schema() {
        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .is_none()
        {
            panic!(
                "IMAGEDB_POSTGRES_BIN is not set; cannot run the real external PostgreSQL test.                  Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run                  `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime                  at .local/db-tools/postgresql-18.4/pgsql/bin."
            );
        }

        use crate::infrastructure::postgres::PostgresManager;
        use crate::infrastructure::secrets::CredentialStore;
        use crate::infrastructure::settings::SettingsStore;
        use tempfile::TempDir;

        let service_tmp = TempDir::new().unwrap();
        let target_tmp = TempDir::new().unwrap();
        let settings_tmp = TempDir::new().unwrap();

        let service_manager = Arc::new(Mutex::new(PostgresManager::new(service_tmp.path())));
        let settings = Arc::new(Mutex::new(SettingsStore::new(settings_tmp.path()).unwrap()));
        let credentials =
            Arc::new(CredentialStore::new_file_for_tests(settings_tmp.path()).unwrap());
        let service = DatabaseService::new(service_manager, settings.clone(), credentials);

        let mut target_manager = PostgresManager::new(target_tmp.path());
        let target_probe = target_manager.initialize().await.expect("target init");
        assert!(
            target_probe.connection_ok,
            "target init failed: {:?}",
            target_probe.diagnostics
        );

        let target_config = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: target_manager.port(),
            database: target_manager.database().to_string(),
            username: target_manager.username().to_string(),
            password: target_manager.password().map(ToOwned::to_owned),
            tls_mode: TlsMode::Disable,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            connect_timeout_secs: 10,
            query_timeout_secs: 15,
            profile_name: Some("real-empty-external-test".to_string()),
        };

        let preflight = service
            .test_external_connection(&target_config)
            .await
            .expect("preflight empty external target");
        assert!(preflight.connection_ok, "{:?}", preflight.diagnostics);
        assert!(preflight.version_ok, "{:?}", preflight.diagnostics);
        assert!(preflight.pgvector_available, "{:?}", preflight.diagnostics);
        assert!(
            preflight.can_create_extension,
            "{:?}",
            preflight.diagnostics
        );
        assert!(preflight.can_create_tables, "{:?}", preflight.diagnostics);
        assert!(preflight.can_modify_schema, "{:?}", preflight.diagnostics);
        assert!(preflight.read_write_ok, "{:?}", preflight.diagnostics);
        assert!(preflight.encoding_ok, "{:?}", preflight.diagnostics);
        assert!(preflight.timezone_ok, "{:?}", preflight.diagnostics);
        assert!(preflight.not_read_only, "{:?}", preflight.diagnostics);
        assert!(preflight.migration_state_ok, "{:?}", preflight.diagnostics);
        assert!(preflight.schema_compatible, "{:?}", preflight.diagnostics);
        assert_eq!(preflight.migration_version, None);

        let state = service
            .initialize_external(&target_config)
            .await
            .expect("initialize empty external database");
        assert_eq!(state.mode, Some(DatabaseMode::External));
        assert_eq!(state.status, DatabaseStatus::Connected);
        assert!(state.pgvector_available);
        assert_eq!(
            state.migration_version.as_deref(),
            Some(MigrationRunner::latest_version())
        );

        let (target_client, target_handle) =
            target_manager.connect().await.expect("target reconnect");
        let vector_installed = target_client
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='vector')",
                &[],
            )
            .await
            .expect("inspect vector extension")
            .get::<_, bool>(0);
        assert!(vector_installed);
        let schema_version = MigrationRunner::current_version(&target_client)
            .await
            .expect("current version");
        assert_eq!(
            schema_version.as_deref(),
            Some(MigrationRunner::latest_version())
        );
        let app_meta_exists = target_client
            .query_one("SELECT to_regclass('public.app_meta') IS NOT NULL", &[])
            .await
            .expect("inspect app_meta")
            .get::<_, bool>(0);
        assert!(app_meta_exists);
        target_handle.abort();

        let stored = settings.lock().await;
        assert_eq!(stored.get().database_mode.as_deref(), Some("external"));
        assert_eq!(
            stored.get().external_profile_name.as_deref(),
            Some("real-empty-external-test")
        );
        assert!(
            stored.get().external_host.is_some(),
            "external settings should retain non-secret profile metadata"
        );
        drop(stored);

        target_manager.shutdown().await.expect("target shutdown");
    }

    #[cfg(feature = "real-db-tests")]
    #[tokio::test]
    #[ignore]
    async fn real_external_missing_database_is_created_during_initialize() {
        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .is_none()
        {
            panic!(
                "IMAGEDB_POSTGRES_BIN is not set; cannot run the real external PostgreSQL test.                  Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run                  `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime                  at .local/db-tools/postgresql-18.4/pgsql/bin."
            );
        }

        use crate::infrastructure::postgres::PostgresManager;
        use crate::infrastructure::secrets::CredentialStore;
        use crate::infrastructure::settings::SettingsStore;
        use tempfile::TempDir;

        let service_tmp = TempDir::new().unwrap();
        let target_tmp = TempDir::new().unwrap();
        let settings_tmp = TempDir::new().unwrap();

        let service_manager = Arc::new(Mutex::new(PostgresManager::new(service_tmp.path())));
        let settings = Arc::new(Mutex::new(SettingsStore::new(settings_tmp.path()).unwrap()));
        let credentials =
            Arc::new(CredentialStore::new_file_for_tests(settings_tmp.path()).unwrap());
        let service = DatabaseService::new(service_manager, settings.clone(), credentials);

        let mut target_manager = PostgresManager::new(target_tmp.path());
        let target_probe = target_manager.initialize().await.expect("target init");
        assert!(
            target_probe.connection_ok,
            "target init failed: {:?}",
            target_probe.diagnostics
        );

        let missing_database = format!("imagedb_missing_{}", uuid::Uuid::new_v4().simple());
        let target_config = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: target_manager.port(),
            database: missing_database.clone(),
            username: target_manager.username().to_string(),
            password: target_manager.password().map(ToOwned::to_owned),
            tls_mode: TlsMode::Disable,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            connect_timeout_secs: 10,
            query_timeout_secs: 15,
            profile_name: Some("real-missing-external-test".to_string()),
        };

        let preflight = service
            .test_external_connection(&target_config)
            .await
            .expect("preflight missing external target");
        assert!(!preflight.connection_ok, "{:?}", preflight.diagnostics);

        let state = service
            .initialize_external(&target_config)
            .await
            .expect("initialize missing external database");
        assert_eq!(state.mode, Some(DatabaseMode::External));
        assert_eq!(state.status, DatabaseStatus::Connected);
        assert!(state.pgvector_available);
        assert_eq!(
            state.migration_version.as_deref(),
            Some(MigrationRunner::latest_version())
        );
        assert!(
            state
                .diagnostics
                .iter()
                .any(|d| d.contains("Created external database")),
            "state diagnostics: {:?}",
            state.diagnostics
        );

        let (target_client, target_handle) = connect_external(&target_config)
            .await
            .expect("connect newly created external database");
        let vector_installed = target_client
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='vector')",
                &[],
            )
            .await
            .expect("inspect vector extension")
            .get::<_, bool>(0);
        assert!(vector_installed);
        let schema_version = MigrationRunner::current_version(&target_client)
            .await
            .expect("current version");
        assert_eq!(
            schema_version.as_deref(),
            Some(MigrationRunner::latest_version())
        );
        target_handle.abort();

        let stored = settings.lock().await;
        assert_eq!(stored.get().database_mode.as_deref(), Some("external"));
        assert_eq!(
            stored.get().external_database.as_deref(),
            Some(missing_database.as_str())
        );
        drop(stored);

        target_manager.shutdown().await.expect("target shutdown");
    }

    #[cfg(feature = "real-db-tests")]
    #[tokio::test]
    #[ignore]
    async fn real_external_unreachable_fallback_switches_to_managed_without_touching_external() {
        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .is_none()
        {
            panic!(
                "IMAGEDB_POSTGRES_BIN is not set; cannot run the real external PostgreSQL test.                  Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run                  `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime                  at .local/db-tools/postgresql-18.4/pgsql/bin."
            );
        }

        use crate::infrastructure::postgres::PostgresManager;
        use crate::infrastructure::secrets::CredentialStore;
        use crate::infrastructure::settings::SettingsStore;
        use serde_json::json;
        use tempfile::TempDir;

        let managed_tmp = TempDir::new().unwrap();
        let target_tmp = TempDir::new().unwrap();
        let settings_tmp = TempDir::new().unwrap();

        let managed_manager = Arc::new(Mutex::new(PostgresManager::new(managed_tmp.path())));
        let settings = Arc::new(Mutex::new(SettingsStore::new(settings_tmp.path()).unwrap()));
        let credentials =
            Arc::new(CredentialStore::new_file_for_tests(settings_tmp.path()).unwrap());
        let service = DatabaseService::new(managed_manager.clone(), settings.clone(), credentials);

        let managed_state = service.initialize_managed().await.expect("managed init");
        assert_eq!(managed_state.mode, Some(DatabaseMode::ManagedLocal));
        assert_eq!(managed_state.status, DatabaseStatus::Connected);

        let mut target_manager = PostgresManager::new(target_tmp.path());
        let target_probe = target_manager.initialize().await.expect("target init");
        assert!(
            target_probe.connection_ok,
            "target init failed: {:?}",
            target_probe.diagnostics
        );

        let target_config = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: target_manager.port(),
            database: target_manager.database().to_string(),
            username: target_manager.username().to_string(),
            password: target_manager.password().map(ToOwned::to_owned),
            tls_mode: TlsMode::Disable,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            connect_timeout_secs: 1,
            query_timeout_secs: 5,
            profile_name: Some("real-fallback-external-test".to_string()),
        };

        let external_state = service
            .initialize_external(&target_config)
            .await
            .expect("initialize external target");
        assert_eq!(external_state.mode, Some(DatabaseMode::External));
        assert_eq!(external_state.status, DatabaseStatus::Connected);

        let (target_client, target_handle) =
            target_manager.connect().await.expect("target connect");
        target_client
            .execute(
                "INSERT INTO app_meta (key, value) VALUES ($1, $2)
                 ON CONFLICT (key) DO UPDATE SET value = $2",
                &[&"m7_fallback_probe", &json!({"external": true})],
            )
            .await
            .expect("seed external app_meta");
        target_handle.abort();

        target_manager.shutdown().await.expect("target shutdown");

        let unreachable = service.get_state().await;
        assert_eq!(unreachable.mode, Some(DatabaseMode::External));
        assert!(
            matches!(unreachable.status, DatabaseStatus::Error(_)),
            "expected unreachable external error, got {:?}",
            unreachable.status
        );
        assert!(
            unreachable.diagnostics.iter().any(|d| {
                d.contains("controlled switch-to-managed action")
                    && d.contains("without modifying external data")
            }),
            "missing controlled fallback diagnostic: {:?}",
            unreachable.diagnostics
        );

        let fallback_state = service
            .switch_to_managed()
            .await
            .expect("controlled switch to managed");
        assert_eq!(fallback_state.mode, Some(DatabaseMode::ManagedLocal));
        assert_eq!(
            fallback_state.status,
            DatabaseStatus::Connected,
            "managed fallback diagnostics: {:?}",
            fallback_state.diagnostics
        );
        assert!(fallback_state.pgvector_available);

        let stored = settings.lock().await;
        assert_eq!(stored.get().database_mode.as_deref(), Some("managed_local"));
        assert_eq!(
            stored.get().external_profile_name.as_deref(),
            Some("real-fallback-external-test")
        );
        assert!(
            stored.get().external_host.is_some(),
            "controlled fallback should retain external profile metadata for later recovery"
        );
        drop(stored);

        let restart_probe = target_manager.initialize().await.expect("target restart");
        assert!(
            restart_probe.connection_ok,
            "target restart failed: {:?}",
            restart_probe.diagnostics
        );
        let (target_client, target_handle) =
            target_manager.connect().await.expect("target reconnect");
        let external_probe = target_client
            .query_one(
                "SELECT value FROM app_meta WHERE key = 'm7_fallback_probe'",
                &[],
            )
            .await
            .expect("external probe row still exists")
            .get::<_, serde_json::Value>(0);
        assert_eq!(external_probe, json!({"external": true}));
        target_handle.abort();

        managed_manager
            .lock()
            .await
            .shutdown()
            .await
            .expect("managed shutdown");
        target_manager.shutdown().await.expect("target shutdown");
    }

    #[cfg(feature = "real-db-tests")]
    #[tokio::test]
    #[ignore]
    async fn real_external_existing_database_upgrades_from_old_schema() {
        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .is_none()
        {
            panic!(
                "IMAGEDB_POSTGRES_BIN is not set; cannot run the real external PostgreSQL test.                  Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run                  `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime                  at .local/db-tools/postgresql-18.4/pgsql/bin."
            );
        }

        use crate::infrastructure::postgres::PostgresManager;
        use crate::infrastructure::secrets::CredentialStore;
        use crate::infrastructure::settings::SettingsStore;
        use tempfile::TempDir;

        const MIGRATION_0001: &str = include_str!("../../migrations/0001_initial.sql");

        let target_tmp = TempDir::new().unwrap();
        let settings_tmp = TempDir::new().unwrap();
        let mut target_manager = PostgresManager::new(target_tmp.path());
        let target_probe = target_manager.initialize().await.expect("target init");
        assert!(
            target_probe.connection_ok,
            "target init failed: {:?}",
            target_probe.diagnostics
        );

        let (target_client, target_handle) =
            target_manager.connect().await.expect("target connect");
        target_client
            .batch_execute(MIGRATION_0001)
            .await
            .expect("apply migration 0001");
        target_client
            .batch_execute(
                "CREATE TABLE schema_migrations (
                    version TEXT PRIMARY KEY,
                    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
                );
                INSERT INTO schema_migrations (version) VALUES ('0001_initial');",
            )
            .await
            .expect("seed old migration history");
        target_handle.abort();

        let manager = Arc::new(Mutex::new(PostgresManager::new(settings_tmp.path())));
        let settings = Arc::new(Mutex::new(SettingsStore::new(settings_tmp.path()).unwrap()));
        let credentials =
            Arc::new(CredentialStore::new_file_for_tests(settings_tmp.path()).unwrap());
        let service = DatabaseService::new(manager, settings, credentials);
        let target_config = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: target_manager.port(),
            database: target_manager.database().to_string(),
            username: target_manager.username().to_string(),
            password: target_manager.password().map(ToOwned::to_owned),
            tls_mode: TlsMode::Disable,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            connect_timeout_secs: 10,
            query_timeout_secs: 15,
            profile_name: Some("real-existing-upgrade-test".to_string()),
        };

        let preflight = service
            .test_external_connection(&target_config)
            .await
            .expect("preflight");
        assert!(
            preflight.schema_compatible,
            "preflight diagnostics: {:?}",
            preflight.diagnostics
        );
        assert_eq!(preflight.migration_version.as_deref(), Some("0001_initial"));

        let state = service
            .initialize_external(&target_config)
            .await
            .expect("initialize external old database");
        assert_eq!(state.mode, Some(DatabaseMode::External));
        assert_eq!(
            state.migration_version.as_deref(),
            Some(MigrationRunner::latest_version())
        );

        let (upgraded_client, upgraded_handle) =
            target_manager.connect().await.expect("target reconnect");
        let source_snapshot_hash_exists = upgraded_client
            .query_one(
                "SELECT EXISTS (
                    SELECT 1
                    FROM information_schema.columns
                    WHERE table_schema = 'public'
                      AND table_name = 'import_albums'
                      AND column_name = 'source_snapshot_hash'
                )",
                &[],
            )
            .await
            .expect("inspect upgraded column")
            .get::<_, bool>(0);
        assert!(
            !source_snapshot_hash_exists,
            "migration 0009 should drop import_albums.source_snapshot_hash"
        );
        upgraded_handle.abort();
        target_manager.shutdown().await.expect("target shutdown");
    }

    #[cfg(feature = "real-db-tests")]
    #[tokio::test]
    #[ignore]
    async fn real_external_existing_database_rejects_unknown_future_migration() {
        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .is_none()
        {
            panic!(
                "IMAGEDB_POSTGRES_BIN is not set; cannot run the real external PostgreSQL test.                  Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run                  `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime                  at .local/db-tools/postgresql-18.4/pgsql/bin."
            );
        }

        use crate::infrastructure::postgres::PostgresManager;
        use crate::infrastructure::secrets::CredentialStore;
        use crate::infrastructure::settings::SettingsStore;
        use tempfile::TempDir;

        let target_tmp = TempDir::new().unwrap();
        let settings_tmp = TempDir::new().unwrap();
        let mut target_manager = PostgresManager::new(target_tmp.path());
        let target_probe = target_manager.initialize().await.expect("target init");
        assert!(
            target_probe.connection_ok,
            "target init failed: {:?}",
            target_probe.diagnostics
        );

        let (target_client, target_handle) =
            target_manager.connect().await.expect("target connect");
        target_client
            .batch_execute(
                "CREATE TABLE schema_migrations (
                    version TEXT PRIMARY KEY,
                    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
                );
                INSERT INTO schema_migrations (version) VALUES ('9999_future');",
            )
            .await
            .expect("seed future migration history");
        target_handle.abort();

        let manager = Arc::new(Mutex::new(PostgresManager::new(settings_tmp.path())));
        let settings = Arc::new(Mutex::new(SettingsStore::new(settings_tmp.path()).unwrap()));
        let credentials =
            Arc::new(CredentialStore::new_file_for_tests(settings_tmp.path()).unwrap());
        let service = DatabaseService::new(manager, settings, credentials);
        let target_config = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: target_manager.port(),
            database: target_manager.database().to_string(),
            username: target_manager.username().to_string(),
            password: target_manager.password().map(ToOwned::to_owned),
            tls_mode: TlsMode::Disable,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            connect_timeout_secs: 10,
            query_timeout_secs: 15,
            profile_name: Some("real-future-migration-test".to_string()),
        };

        let preflight = service
            .test_external_connection(&target_config)
            .await
            .expect("preflight");
        assert!(!preflight.migration_state_ok);
        assert!(!preflight.schema_compatible);
        assert_eq!(preflight.migration_version.as_deref(), Some("9999_future"));
        assert!(
            preflight
                .diagnostics
                .iter()
                .any(|d| d.contains("unknown ImageDB migration version")),
            "preflight diagnostics: {:?}",
            preflight.diagnostics
        );

        let state = service
            .initialize_external(&target_config)
            .await
            .expect("initialize external future database");
        assert!(matches!(state.status, DatabaseStatus::Error(_)));
        assert_ne!(state.mode, Some(DatabaseMode::ManagedLocal));

        target_manager.shutdown().await.expect("target shutdown");
    }

    #[cfg(feature = "real-db-tests")]
    #[tokio::test]
    #[ignore]
    async fn real_migrate_managed_to_external_switches_after_row_verification() {
        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .is_none()
        {
            panic!(
                "IMAGEDB_POSTGRES_BIN is not set; cannot run the real external PostgreSQL test.                  Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run                  `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime                  at .local/db-tools/postgresql-18.4/pgsql/bin."
            );
        }

        use crate::infrastructure::postgres::PostgresManager;
        use crate::infrastructure::secrets::CredentialStore;
        use crate::infrastructure::settings::SettingsStore;
        use serde_json::json;
        use tempfile::TempDir;

        let source_tmp = TempDir::new().unwrap();
        let target_tmp = TempDir::new().unwrap();
        let source_settings_tmp = TempDir::new().unwrap();

        let source_manager = Arc::new(Mutex::new(PostgresManager::new(source_tmp.path())));
        let source_settings = Arc::new(Mutex::new(
            SettingsStore::new(source_settings_tmp.path()).unwrap(),
        ));
        let credentials =
            Arc::new(CredentialStore::new_file_for_tests(source_settings_tmp.path()).unwrap());
        let service =
            DatabaseService::new(source_manager.clone(), source_settings.clone(), credentials);

        let managed_state = service.initialize_managed().await.expect("managed init");
        assert_eq!(managed_state.mode, Some(DatabaseMode::ManagedLocal));

        let (source_client, source_handle) = {
            let mgr = source_manager.lock().await;
            mgr.connect().await.expect("source connect")
        };
        source_client
            .execute(
                "INSERT INTO app_meta (key, value) VALUES ($1, $2)
                 ON CONFLICT (key) DO UPDATE SET value = $2",
                &[&"m7_migration_probe", &json!({"ok": true})],
            )
            .await
            .expect("seed app_meta");
        source_handle.abort();

        let mut target_manager = PostgresManager::new(target_tmp.path());
        let target_probe = target_manager.initialize().await.expect("target init");
        assert!(
            target_probe.connection_ok,
            "target init failed: {:?}",
            target_probe.diagnostics
        );

        let target_config = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: target_manager.port(),
            database: target_manager.database().to_string(),
            username: target_manager.username().to_string(),
            password: target_manager.password().map(ToOwned::to_owned),
            tls_mode: TlsMode::Disable,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            connect_timeout_secs: 10,
            query_timeout_secs: 15,
            profile_name: Some("real-migration-test".to_string()),
        };

        let result = service
            .migrate_managed_to_external(&target_config)
            .await
            .expect("migration result");
        assert!(
            result.switched,
            "migration diagnostics: {:?}",
            result.diagnostics
        );
        let backup_path = result.backup_path.as_ref().expect("backup path");
        assert!(std::path::Path::new(backup_path).is_file());
        assert!(result.row_counts.iter().all(|count| count.matches));
        assert!(result
            .row_counts
            .iter()
            .any(|count| count.table == "app_meta" && count.external_rows >= 1));
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.contains("table content fingerprints verified")),
            "missing content verification diagnostic: {:?}",
            result.diagnostics
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.contains("constraints and indexes verified")),
            "missing schema verification diagnostic: {:?}",
            result.diagnostics
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.contains("read/write smoke verified")),
            "missing read/write smoke diagnostic: {:?}",
            result.diagnostics
        );
        let backup_sql = std::fs::read_to_string(backup_path).expect("read backup sql");
        assert!(backup_sql.contains("m7_migration_probe"));

        let (target_client, target_handle) =
            target_manager.connect().await.expect("target connect");
        let migrated = target_client
            .query_one(
                "SELECT value FROM app_meta WHERE key = 'm7_migration_probe'",
                &[],
            )
            .await
            .expect("query migrated app_meta")
            .get::<_, serde_json::Value>(0);
        assert_eq!(migrated, json!({"ok": true}));
        target_handle.abort();

        source_manager
            .lock()
            .await
            .shutdown()
            .await
            .expect("source shutdown");
        target_manager.shutdown().await.expect("target shutdown");
    }
}

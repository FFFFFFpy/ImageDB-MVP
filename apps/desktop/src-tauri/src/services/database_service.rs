use crate::domain::{
    ConnectionConfig, DatabaseMode, DatabaseState, DatabaseStatus, ExternalCheckResult,
    ExternalPreflightCheck, ManagedDbConfig, TlsMode,
};
use crate::error::AppError;
use crate::infrastructure::postgres::{connect_external, MigrationRunner, PostgresManager};
use crate::infrastructure::secrets::{external_profile_key, CredentialStore};
use crate::infrastructure::settings::AppSettings;
use crate::infrastructure::settings::SettingsStore;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

pub struct DatabaseService {
    postgres_manager: Arc<Mutex<PostgresManager>>,
    settings: Arc<Mutex<SettingsStore>>,
    credentials: Arc<CredentialStore>,
}

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
                            vec![e.to_string()],
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
                        "PostgreSQL binaries not found on this system".to_string(),
                    ),
                    false,
                    None,
                    mgr.diagnostics().to_vec(),
                )
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
                    "PostgreSQL binaries not found. Install PostgreSQL or place binaries alongside the application.".to_string(),
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

        if config.client_cert_path.is_some() || config.client_key_path.is_some() {
            checks.push(ExternalPreflightCheck::warn(
                "tls.client_certificate",
                "Client certificate paths are recorded for profile review; this build does not yet load client key pairs",
            ));
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

        let migration_version = timeout(
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

        let migration_version = if migration_version.is_some() {
            MigrationRunner::current_version(&client)
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        let migration_state_ok = timeout(
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

        let schema_compatible = migration_state_ok;
        checks.push(if migration_state_ok {
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
        let check = self.test_external_connection(&effective_config).await?;

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
                diagnostics: check.diagnostics,
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

        let mut settings = self.settings.lock().await;
        let current = settings.get().clone();
        if let Some(password) = &effective_config.password {
            let key = external_profile_key(
                &effective_config.host,
                effective_config.port,
                &effective_config.database,
                &effective_config.username,
            );
            self.credentials.store(&key, password)?;
        }
        settings.update(crate::infrastructure::settings::AppSettings {
            database_mode: Some("external".to_string()),
            external_host: Some(effective_config.host.clone()),
            external_port: Some(effective_config.port),
            external_database: Some(effective_config.database.clone()),
            external_username: Some(effective_config.username.clone()),
            external_tls_mode: Some(effective_config.tls_mode.as_str().to_string()),
            external_ca_cert_path: effective_config.ca_cert_path.clone(),
            external_client_cert_path: effective_config.client_cert_path.clone(),
            external_client_key_path: effective_config.client_key_path.clone(),
            external_connect_timeout_secs: Some(effective_config.connect_timeout_secs),
            external_query_timeout_secs: Some(effective_config.query_timeout_secs),
            external_profile_name: effective_config.profile_name.clone(),
            first_run_completed: true,
            ..current
        })?;
        let mut mgr = self.postgres_manager.lock().await;
        mgr.use_external_profile(effective_config.clone());

        Ok(DatabaseState {
            mode: Some(DatabaseMode::External),
            status: DatabaseStatus::Connected,
            managed_config: None,
            external_config: Some(Self::redacted_config(&effective_config)),
            pgvector_available: check.pgvector_available,
            migration_version: version,
            diagnostics: {
                let mut d = check.diagnostics;
                if !applied.is_empty() {
                    d.push(format!("Applied migrations: {}", applied.join(", ")));
                }
                d
            },
        })
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
}

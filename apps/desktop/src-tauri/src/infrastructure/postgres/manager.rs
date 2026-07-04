use crate::domain::ConnectionConfig;
use crate::error::AppError;
use crate::infrastructure::postgres::connect_external;
use rand::Rng;
use serde::Serialize;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

const PORT_FILE: &str = "postgres_port";
const CREDENTIAL_FILE: &str = "postgres_credentials";
const DEFAULT_USERNAME: &str = "imagedb";
const DEFAULT_DATABASE: &str = "imagedb";

#[derive(Debug, Clone, Serialize)]
pub struct PostgresProbeResult {
    pub available: bool,
    pub managed: bool,
    pub pgvector_available: bool,
    pub port: Option<u16>,
    pub data_dir: Option<String>,
    pub database_created: bool,
    pub connection_ok: bool,
    pub diagnostics: Vec<String>,
}

pub struct PostgresManager {
    data_dir: PathBuf,
    port: u16,
    username: String,
    database: String,
    password: Option<String>,
    pg_ctl: Option<PathBuf>,
    initdb: Option<PathBuf>,
    psql: Option<PathBuf>,
    pg_dump: Option<PathBuf>,
    diagnostics: Vec<String>,
    server_running: bool,
    active_external: Option<ConnectionConfig>,
}

impl PostgresManager {
    pub fn new(app_data_dir: &std::path::Path) -> Self {
        let data_dir = app_data_dir.join("postgres_data");
        let mut mgr = Self {
            data_dir,
            port: 0,
            username: DEFAULT_USERNAME.to_string(),
            database: DEFAULT_DATABASE.to_string(),
            password: None,
            pg_ctl: None,
            initdb: None,
            psql: None,
            pg_dump: None,
            diagnostics: Vec::new(),
            server_running: false,
            active_external: None,
        };
        mgr.locate_binaries();
        mgr.load_saved_config();
        mgr
    }

    fn locate_binaries(&mut self) {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));

        let exe_suffix = if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        };

        if let Ok(runtime_dir) = std::env::var("IMAGEDB_POSTGRES_RUNTIME_DIR") {
            let runtime = PathBuf::from(&runtime_dir);
            let bin_dir = runtime.join("bin");
            if self.try_use_bin_dir(&bin_dir, exe_suffix) {
                self.diagnostics.push(format!(
                    "Found bundled PostgreSQL runtime via IMAGEDB_POSTGRES_RUNTIME_DIR: {}",
                    runtime.display()
                ));
                return;
            }
            self.diagnostics.push(format!(
                "IMAGEDB_POSTGRES_RUNTIME_DIR='{}' is set but missing bin/pg_ctl/initdb/psql; \
                 falling back to default search",
                runtime.display()
            ));
        }

        if let Ok(env_bin) = std::env::var("IMAGEDB_POSTGRES_BIN") {
            let base = PathBuf::from(&env_bin);
            if self.try_use_bin_dir(&base, exe_suffix) {
                self.diagnostics.push(format!(
                    "Found PostgreSQL binaries via IMAGEDB_POSTGRES_BIN: {}",
                    base.display()
                ));
                return;
            }
            self.diagnostics.push(format!(
                "IMAGEDB_POSTGRES_BIN='{}' is set but missing pg_ctl/initdb/psql; \
                 falling back to default search",
                base.display()
            ));
        }

        let mut search_paths: Vec<PathBuf> = vec![
            {
                let mut p = exe_dir.clone().unwrap_or_default();
                p.push("resources");
                p.push("postgres-runtime");
                p.push("bin");
                p
            },
            {
                let mut p = exe_dir.clone().unwrap_or_default();
                p.push("postgres-runtime");
                p.push("bin");
                p
            },
            {
                let mut p = exe_dir.clone().unwrap_or_default();
                p.push("postgres");
                p
            },
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/usr/bin"),
            #[cfg(target_os = "windows")]
            PathBuf::from("C:\\Program Files\\PostgreSQL\\17\\bin"),
            #[cfg(target_os = "windows")]
            PathBuf::from("C:\\Program Files\\PostgreSQL\\16\\bin"),
        ];
        search_paths.retain(|p| !p.as_os_str().is_empty());

        for base in &search_paths {
            if self.try_use_bin_dir(base, exe_suffix) {
                self.diagnostics
                    .push(format!("Found PostgreSQL binaries at: {}", base.display()));
                return;
            }
        }

        for name in ["pg_ctl", "initdb", "psql", "pg_dump"] {
            let cmd = if cfg!(target_os = "windows") {
                "where"
            } else {
                "which"
            };
            if let Ok(output) = std::process::Command::new(cmd).arg(name).output() {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !path.is_empty() {
                        let pb = PathBuf::from(&path);
                        match name {
                            "pg_ctl" => self.pg_ctl = Some(pb),
                            "initdb" => self.initdb = Some(pb),
                            "psql" => self.psql = Some(pb),
                            "pg_dump" => self.pg_dump = Some(pb),
                            _ => {}
                        }
                    }
                }
            }

            if let Some(psql) = &self.psql {
                if self.pg_dump.is_none() {
                    let pg_dump = psql.with_file_name(format!("pg_dump{exe_suffix}"));
                    if pg_dump.exists() {
                        self.pg_dump = Some(pg_dump);
                    }
                }
            }
        }

        if self.pg_ctl.is_some() && self.initdb.is_some() && self.psql.is_some() {
            self.diagnostics
                .push("Found PostgreSQL binaries via PATH".to_string());
        } else {
            // A packaged release ships its own PostgreSQL runtime; an end
            // user should never be told to install PostgreSQL themselves.
            // Point them at a reinstall of the application instead.
            self.diagnostics.push(format!(
                "PostgreSQL runtime not found. The application install is incomplete \
                 (missing bundled postgres-runtime/bin). Reinstall ImageDB; do not \
                 install PostgreSQL separately. Searched bundled runtime, {}, \
                 /usr/local/bin, /usr/bin, and PATH.",
                exe_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<unknown exe dir>".to_string())
            ));
        }
    }

    pub(crate) fn try_use_bin_dir(&mut self, base: &std::path::Path, exe_suffix: &str) -> bool {
        let pg_ctl = base.join(format!("pg_ctl{exe_suffix}"));
        let initdb = base.join(format!("initdb{exe_suffix}"));
        let psql = base.join(format!("psql{exe_suffix}"));
        let pg_dump = base.join(format!("pg_dump{exe_suffix}"));
        if pg_ctl.exists() && initdb.exists() && psql.exists() {
            self.pg_ctl = Some(pg_ctl);
            self.initdb = Some(initdb);
            self.psql = Some(psql);
            self.pg_dump = pg_dump.exists().then_some(pg_dump);
            true
        } else {
            false
        }
    }

    fn load_saved_config(&mut self) {
        if let Some(port) = self.read_saved_port() {
            self.port = port;
        }
        if let Some((username, password)) = self.read_saved_credentials() {
            self.username = username;
            self.password = Some(password);
        }
    }

    pub fn binaries_available(&self) -> bool {
        self.pg_ctl.is_some() && self.initdb.is_some() && self.psql.is_some()
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    pub fn psql_path(&self) -> Option<&std::path::Path> {
        self.psql.as_deref()
    }

    pub fn pg_dump_path(&self) -> Option<&std::path::Path> {
        self.pg_dump.as_deref()
    }

    pub fn app_data_dir(&self) -> Option<&std::path::Path> {
        self.data_dir.parent()
    }

    pub fn is_server_running(&self) -> bool {
        self.server_running
    }

    pub fn use_external_profile(&mut self, config: ConnectionConfig) {
        self.active_external = Some(config);
    }

    pub fn use_managed_profile(&mut self) {
        self.active_external = None;
    }

    pub fn active_external_profile(&self) -> Option<&ConnectionConfig> {
        self.active_external.as_ref()
    }

    pub fn connection_string(&self) -> String {
        self.connection_string_for_database(&self.database)
    }

    fn connection_string_for_database(&self, database: &str) -> String {
        let mut parts = vec![
            format!("host=127.0.0.1"),
            format!("port={}", self.port),
            format!("user={}", self.username),
            format!("dbname={database}"),
        ];
        if let Some(ref pw) = self.password {
            parts.push(format!("password={pw}"));
        }
        parts.join(" ")
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    fn find_free_port() -> Result<u16, AppError> {
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|e| AppError::Internal(e.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| AppError::Internal(e.to_string()))?
            .port();
        Ok(port)
    }

    fn generate_password() -> String {
        let mut rng = rand::thread_rng();
        let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
            .chars()
            .collect();
        (0..24)
            .map(|_| chars[rng.gen_range(0..chars.len())])
            .collect()
    }

    fn port_file_path(&self) -> PathBuf {
        self.data_dir.join(PORT_FILE)
    }

    fn credential_file_path(&self) -> PathBuf {
        self.data_dir.join(CREDENTIAL_FILE)
    }

    fn save_port(&self) -> Result<(), AppError> {
        let path = self.port_file_path();
        std::fs::write(&path, self.port.to_string()).map_err(|e| {
            AppError::Internal(format!("cannot write port file {}: {e}", path.display()))
        })
    }

    fn read_saved_port(&self) -> Option<u16> {
        let path = self.port_file_path();
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u16>().ok())
    }

    fn read_saved_credentials(&self) -> Option<(String, String)> {
        let path = self.credential_file_path();
        let content = std::fs::read_to_string(&path).ok()?;
        let parts: Vec<&str> = content.trim().splitn(2, ':').collect();
        if parts.len() == 2 && !parts[0].is_empty() {
            Some((parts[0].to_string(), parts[1].to_string()))
        } else {
            None
        }
    }

    fn path_to_str(path: &std::path::Path) -> Result<String, AppError> {
        path.to_str().map(|s| s.to_string()).ok_or_else(|| {
            AppError::Internal(format!(
                "path contains non-UTF8 characters: {}",
                path.display()
            ))
        })
    }

    fn cluster_files_exist(&self) -> bool {
        self.data_dir.join("PG_VERSION").is_file()
            && self.data_dir.join("base").is_dir()
            && self.data_dir.join("global").is_dir()
            && self.data_dir.join("pg_wal").is_dir()
    }

    pub async fn initialize(&mut self) -> Result<PostgresProbeResult, AppError> {
        if !self.binaries_available() {
            return Ok(PostgresProbeResult {
                available: false,
                managed: false,
                pgvector_available: false,
                port: None,
                data_dir: None,
                database_created: false,
                connection_ok: false,
                diagnostics: self.diagnostics.clone(),
            });
        }

        if !self.data_dir.exists() {
            let parent_dir = self.data_dir.parent().ok_or_else(|| {
                AppError::Internal(format!(
                    "data directory {} has no parent",
                    self.data_dir.display()
                ))
            })?;
            std::fs::create_dir_all(parent_dir).map_err(|e| {
                AppError::Internal(format!(
                    "cannot create parent directory {}: {e}",
                    parent_dir.display()
                ))
            })?;

            self.port = Self::find_free_port()?;
            self.password = Some(Self::generate_password());

            // initdb requires its target data directory to be empty. Our port
            // and credential files must therefore live in a sibling staging
            // directory until initdb finishes.
            let staging_dir = parent_dir.join("postgres_staging");
            if staging_dir.exists() {
                std::fs::remove_dir_all(&staging_dir).map_err(|e| {
                    AppError::Internal(format!(
                        "cannot clean staging directory {}: {e}",
                        staging_dir.display()
                    ))
                })?;
            }
            std::fs::create_dir_all(&staging_dir).map_err(|e| {
                AppError::Internal(format!(
                    "cannot create staging directory {}: {e}",
                    staging_dir.display()
                ))
            })?;

            let staged_port = staging_dir.join(PORT_FILE);
            let staged_credentials = staging_dir.join(CREDENTIAL_FILE);
            std::fs::write(&staged_port, self.port.to_string()).map_err(|e| {
                AppError::Internal(format!(
                    "cannot write staged port file {}: {e}",
                    staged_port.display()
                ))
            })?;
            let password_for_staging = self.password.clone().unwrap_or_default();
            let staged_credential_content = format!("{}:{}", self.username, password_for_staging);
            std::fs::write(&staged_credentials, staged_credential_content).map_err(|e| {
                AppError::Internal(format!(
                    "cannot write staged credential file {}: {e}",
                    staged_credentials.display()
                ))
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &staged_credentials,
                    std::fs::Permissions::from_mode(0o600),
                );
            }

            let Some(initdb) = self.initdb.as_ref() else {
                let _ = std::fs::remove_dir_all(&staging_dir);
                self.diagnostics
                    .push("initdb binary missing during initialization".to_string());
                return Ok(PostgresProbeResult {
                    available: false,
                    managed: false,
                    pgvector_available: false,
                    port: Some(self.port),
                    data_dir: Some(self.data_dir.display().to_string()),
                    database_created: false,
                    connection_ok: false,
                    diagnostics: self.diagnostics.clone(),
                });
            };

            let password = self.password.clone().unwrap_or_default();
            let pwfile = staging_dir.join("initdb_pwfile");
            if let Err(e) = std::fs::write(&pwfile, password.as_bytes()) {
                let _ = std::fs::remove_dir_all(&staging_dir);
                self.diagnostics.push(format!(
                    "cannot write initdb pwfile {}: {e}",
                    pwfile.display()
                ));
                return Ok(PostgresProbeResult {
                    available: true,
                    managed: false,
                    pgvector_available: false,
                    port: Some(self.port),
                    data_dir: Some(self.data_dir.display().to_string()),
                    database_created: false,
                    connection_ok: false,
                    diagnostics: self.diagnostics.clone(),
                });
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&pwfile, std::fs::Permissions::from_mode(0o600));
            }

            let data_dir_str = Self::path_to_str(&self.data_dir)?;
            let pwfile_str = pwfile.display().to_string();
            let output = Command::new(initdb)
                .args([
                    "-D",
                    &data_dir_str,
                    "--no-locale",
                    "--encoding=UTF8",
                    "--auth=md5",
                    &format!("--username={}", self.username),
                    &format!("--pwfile={pwfile_str}"),
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await?;

            let _remove_pw = std::fs::remove_file(&pwfile);

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                // Restore the empty-directory contract so a retry can call
                // initdb again without manual cleanup.
                let _ = std::fs::remove_dir_all(&self.data_dir);
                let _ = std::fs::remove_dir_all(&staging_dir);
                self.diagnostics.push(format!("initdb failed: {stderr}"));
                return Ok(PostgresProbeResult {
                    available: true,
                    managed: false,
                    pgvector_available: false,
                    port: Some(self.port),
                    data_dir: Some(self.data_dir.display().to_string()),
                    database_created: false,
                    connection_ok: false,
                    diagnostics: self.diagnostics.clone(),
                });
            }
            self.diagnostics
                .push("initdb completed successfully".to_string());

            if !self.cluster_files_exist() {
                let _ = std::fs::remove_dir_all(&self.data_dir);
                let _ = std::fs::remove_dir_all(&staging_dir);
                self.diagnostics.push(format!(
                    "initdb did not create a complete PostgreSQL cluster in {}",
                    self.data_dir.display()
                ));
                return Ok(PostgresProbeResult {
                    available: true,
                    managed: false,
                    pgvector_available: false,
                    port: Some(self.port),
                    data_dir: Some(self.data_dir.display().to_string()),
                    database_created: false,
                    connection_ok: false,
                    diagnostics: self.diagnostics.clone(),
                });
            }

            let final_port = self.data_dir.join(PORT_FILE);
            let final_credentials = self.data_dir.join(CREDENTIAL_FILE);
            std::fs::rename(&staged_port, &final_port).map_err(|e| {
                AppError::Internal(format!("cannot move port file into data dir: {e}"))
            })?;
            std::fs::rename(&staged_credentials, &final_credentials).map_err(|e| {
                AppError::Internal(format!("cannot move credential file into data dir: {e}"))
            })?;
            let _ = std::fs::remove_dir_all(&staging_dir);

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &final_credentials,
                    std::fs::Permissions::from_mode(0o600),
                );
            }
        } else {
            if self.port == 0 {
                self.port = self
                    .read_saved_port()
                    .unwrap_or_else(|| Self::find_free_port().unwrap_or(0));
                if self.port == 0 {
                    self.diagnostics
                        .push("Cannot determine port for existing data directory".to_string());
                    return Ok(PostgresProbeResult {
                        available: true,
                        managed: false,
                        pgvector_available: false,
                        port: None,
                        data_dir: Some(self.data_dir.display().to_string()),
                        database_created: false,
                        connection_ok: false,
                        diagnostics: self.diagnostics.clone(),
                    });
                }
                self.save_port()?;
            }
            self.diagnostics.push(format!(
                "Data directory already exists, reusing: {} (port {})",
                self.data_dir.display(),
                self.port
            ));
        }

        self.start_server().await
    }

    pub async fn start_server(&mut self) -> Result<PostgresProbeResult, AppError> {
        let Some(pg_ctl) = self.pg_ctl.as_ref() else {
            self.diagnostics
                .push("pg_ctl binary missing during server startup".to_string());
            return Ok(PostgresProbeResult {
                available: false,
                managed: false,
                pgvector_available: false,
                port: Some(self.port),
                data_dir: Some(self.data_dir.display().to_string()),
                database_created: false,
                connection_ok: false,
                diagnostics: self.diagnostics.clone(),
            });
        };
        let port_str = self.port.to_string();
        let data_dir_str = Self::path_to_str(&self.data_dir)?;
        let log_file = self.data_dir.join("postgres.log");
        let log_file_str = Self::path_to_str(&log_file)?;

        let listen_opts = format!("-p {port_str} -h 127.0.0.1");

        let status = Command::new(pg_ctl)
            .args([
                "start",
                "-D",
                &data_dir_str,
                "-l",
                &log_file_str,
                "-o",
                &listen_opts,
                "-w",
                "-t",
                "10",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;

        if !status.success() {
            let log_tail = std::fs::read_to_string(&log_file)
                .ok()
                .map(|s| s.lines().rev().take(20).collect::<Vec<_>>().join("\n"))
                .unwrap_or_else(|| "<no postgres log output>".to_string());
            if log_tail.contains("already running")
                || log_tail.contains("already started")
                || log_tail.contains("ready to accept connections")
            {
                self.diagnostics.push(
                    "PostgreSQL server is already running or became ready after start warning"
                        .to_string(),
                );
                self.server_running = true;
            } else {
                self.diagnostics
                    .push(format!("pg_ctl start failed; log tail: {log_tail}"));
                return Ok(PostgresProbeResult {
                    available: true,
                    managed: false,
                    pgvector_available: false,
                    port: Some(self.port),
                    data_dir: Some(self.data_dir.display().to_string()),
                    database_created: false,
                    connection_ok: false,
                    diagnostics: self.diagnostics.clone(),
                });
            }
        } else {
            self.diagnostics
                .push(format!("PostgreSQL started on port {port_str}"));
            self.server_running = true;
        }

        let db_created = self.create_database().await;
        let pgvector_ok = self.check_pgvector().await;
        let connection_ok = self.test_connection().await;

        Ok(PostgresProbeResult {
            available: true,
            managed: true,
            pgvector_available: pgvector_ok,
            port: Some(self.port),
            data_dir: Some(self.data_dir.display().to_string()),
            database_created: db_created,
            connection_ok,
            diagnostics: self.diagnostics.clone(),
        })
    }

    async fn create_database(&mut self) -> bool {
        let conn_str = self.connection_string_for_database("postgres");
        let (client, conn) = match timeout(
            Duration::from_secs(15),
            tokio_postgres::connect(&conn_str, tokio_postgres::NoTls),
        )
        .await
        {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => {
                self.diagnostics
                    .push(format!("connect to postgres database failed: {e}"));
                return false;
            }
            Err(_) => {
                self.diagnostics
                    .push("connect to postgres database timed out".to_string());
                return false;
            }
        };
        let handle = tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("PostgreSQL postgres-db connection lost: {e}");
            }
        });

        let exists = match timeout(
            Duration::from_secs(15),
            client.query_one(
                "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)",
                &[&self.database],
            ),
        )
        .await
        {
            Ok(Ok(row)) => Ok(row.get::<_, bool>(0)),
            Err(e) => {
                self.diagnostics
                    .push(format!("database existence check timed out: {e}"));
                handle.abort();
                return false;
            }
            Ok(Err(e)) => Err(e),
        };

        match exists {
            Ok(true) => {
                self.diagnostics
                    .push(format!("Database '{}' already exists", self.database));
                handle.abort();
                true
            }
            Ok(false) => {
                let sql = format!("CREATE DATABASE {}", self.database);
                match timeout(Duration::from_secs(15), client.batch_execute(&sql)).await {
                    Err(_) => {
                        self.diagnostics
                            .push("CREATE DATABASE timed out".to_string());
                        handle.abort();
                        false
                    }
                    Ok(Ok(())) => {
                        self.diagnostics
                            .push(format!("Created database '{}'", self.database));
                        handle.abort();
                        true
                    }
                    Ok(Err(e)) => {
                        self.diagnostics
                            .push(format!("CREATE DATABASE failed: {e}"));
                        handle.abort();
                        false
                    }
                }
            }
            Err(e) => {
                self.diagnostics
                    .push(format!("database existence check failed: {e}"));
                handle.abort();
                false
            }
        }
    }

    pub async fn check_pgvector(&mut self) -> bool {
        let conn_str = self.connection_string();
        let (client, conn) = match timeout(
            Duration::from_secs(15),
            tokio_postgres::connect(&conn_str, tokio_postgres::NoTls),
        )
        .await
        {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => {
                self.diagnostics
                    .push(format!("pgvector check connection failed: {e}"));
                return false;
            }
            Err(_) => {
                self.diagnostics
                    .push("pgvector check connection timed out".to_string());
                return false;
            }
        };
        let handle = tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("PostgreSQL pgvector-check connection lost: {e}");
            }
        });

        match timeout(
            Duration::from_secs(15),
            client.batch_execute("CREATE EXTENSION IF NOT EXISTS vector"),
        )
        .await
        {
            Err(_) => {
                self.diagnostics
                    .push("pgvector extension creation timed out".to_string());
                handle.abort();
                false
            }
            Ok(Ok(())) => {
                self.diagnostics
                    .push("pgvector extension enabled".to_string());
                handle.abort();
                true
            }
            Ok(Err(e)) => {
                self.diagnostics
                    .push(format!("pgvector not available: {e}"));
                handle.abort();
                false
            }
        }
    }

    pub async fn test_connection(&mut self) -> bool {
        let conn_str = self.connection_string();

        match tokio_postgres::connect(&conn_str, tokio_postgres::NoTls).await {
            Ok((client, conn)) => {
                let handle = tokio::spawn(async move {
                    let _ = conn.await;
                });

                match client.query_one("SELECT 1", &[]).await {
                    Ok(_) => {
                        self.diagnostics
                            .push("tokio-postgres connection test: OK".to_string());
                        handle.abort();
                        true
                    }
                    Err(e) => {
                        self.diagnostics
                            .push(format!("Connection test query failed: {e}"));
                        handle.abort();
                        false
                    }
                }
            }
            Err(e) => {
                self.diagnostics
                    .push(format!("tokio-postgres connect failed: {e}"));
                false
            }
        }
    }

    pub async fn connect(
        &self,
    ) -> Result<(tokio_postgres::Client, tokio::task::JoinHandle<()>), AppError> {
        if let Some(config) = &self.active_external {
            return connect_external(config).await;
        }

        let conn_str = self.connection_string();
        let (client, conn) = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
            .await
            .map_err(|e| AppError::PostgresUnavailable(format!("connect failed: {e}")))?;

        let handle = tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::warn!("PostgreSQL connection lost: {e}");
            }
        });

        Ok((client, handle))
    }

    pub async fn shutdown(&mut self) -> Result<(), AppError> {
        if let Some(pg_ctl) = &self.pg_ctl {
            let has_postmaster = self.data_dir.join("postmaster.pid").exists();
            if self.data_dir.exists() && (self.server_running || has_postmaster) {
                let data_dir_str = Self::path_to_str(&self.data_dir)?;
                let output = Command::new(pg_ctl)
                    .args(["stop", "-D", &data_dir_str, "-m", "fast", "-w"])
                    .stdin(Stdio::null())
                    .output()
                    .await?;

                if output.status.success() {
                    self.diagnostics.push("PostgreSQL stopped".to_string());
                    self.server_running = false;
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    self.diagnostics
                        .push(format!("pg_ctl stop warning: {stderr}"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_missing_binaries_diagnostic() {
        let tmp = TempDir::new().unwrap();
        let isolated = tmp.path().join("isolated_app_data");
        std::fs::create_dir_all(&isolated).unwrap();

        let mut mgr = PostgresManager::new(&isolated);
        mgr.pg_ctl = None;
        mgr.initdb = None;
        mgr.psql = None;
        mgr.pg_dump = None;

        assert!(!mgr.binaries_available());
        assert!(
            mgr.diagnostics.iter().any(|d| d.contains("not found")),
            "Expected diagnostic about missing binaries, got: {:?}",
            mgr.diagnostics
        );
        // The user-facing message must guide a reinstall, NOT tell the user
        // to install PostgreSQL themselves (the release ships its own runtime).
        assert!(
            mgr.diagnostics
                .iter()
                .any(|d| d.contains("incomplete") && d.contains("Reinstall")),
            "Expected diagnostic to call the install incomplete and recommend reinstall, got: {:?}",
            mgr.diagnostics
        );
    }

    /// Verify the binary probe accepts a packaged postgres-runtime/bin/ dir
    /// of the shape the release bundles (via Tauri resources →
    /// IMAGEDB_POSTGRES_RUNTIME_DIR → locate_binaries → try_use_bin_dir). A
    /// clean Windows install with only the bundled runtime must be detected
    /// as available without any system PostgreSQL.
    #[test]
    fn test_locator_finds_packaged_runtime_dir() {
        let tmp = TempDir::new().unwrap();
        let isolated = tmp.path().join("isolated_app_data");
        std::fs::create_dir_all(&isolated).unwrap();

        // Simulate the release resource dir:
        // <resources>/postgres-runtime/bin/{pg_ctl,initdb,psql,pg_dump}
        let bin_dir = tmp.path().join("postgres-runtime").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let exe_suffix = if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        };
        for name in ["pg_ctl", "initdb", "psql", "pg_dump"] {
            std::fs::write(bin_dir.join(format!("{name}{exe_suffix}")), b"").unwrap();
        }

        let mut mgr = PostgresManager::new(&isolated);
        // Reset to "nothing found" then exercise the probe the resource-dir
        // path uses, so this test is independent of the host's PostgreSQL.
        mgr.pg_ctl = None;
        mgr.initdb = None;
        mgr.psql = None;
        mgr.pg_dump = None;

        assert!(mgr.try_use_bin_dir(&bin_dir, exe_suffix));
        assert!(mgr.binaries_available());
        assert!(
            mgr.pg_dump.is_some(),
            "pg_dump should be discovered alongside the others"
        );
    }

    /// When the packaged runtime bin/ is missing required binaries, the
    /// probe must reject it so the locator falls back to the default search
    /// (rather than silently succeeding with a partial runtime).
    #[test]
    fn test_locator_rejects_incomplete_runtime_dir() {
        let tmp = TempDir::new().unwrap();
        let isolated = tmp.path().join("isolated_app_data");
        std::fs::create_dir_all(&isolated).unwrap();

        let bin_dir = tmp.path().join("postgres-runtime").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let exe_suffix = if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        };
        // Only pg_ctl + initdb present — psql and pg_dump missing.
        std::fs::write(bin_dir.join(format!("pg_ctl{exe_suffix}")), b"").unwrap();
        std::fs::write(bin_dir.join(format!("initdb{exe_suffix}")), b"").unwrap();

        let mut mgr = PostgresManager::new(&isolated);
        mgr.pg_ctl = None;
        mgr.initdb = None;
        mgr.psql = None;
        mgr.pg_dump = None;

        assert!(!mgr.try_use_bin_dir(&bin_dir, exe_suffix));
        assert!(!mgr.binaries_available());
    }

    #[test]
    fn test_port_persistence_round_trip() {
        let tmp = TempDir::new().unwrap();
        let isolated = tmp.path().join("isolated_app_data");
        std::fs::create_dir_all(&isolated).unwrap();

        let mgr = PostgresManager::new(&isolated);
        std::fs::create_dir_all(&mgr.data_dir).unwrap();

        let port_file = mgr.data_dir.join(PORT_FILE);
        assert!(!port_file.exists(), "port file should not exist initially");

        let mgr = PostgresManager { port: 54321, ..mgr };
        mgr.save_port().unwrap();

        assert!(port_file.exists());
        let contents = std::fs::read_to_string(&port_file).unwrap();
        assert_eq!(contents, "54321");

        let loaded = mgr.read_saved_port();
        assert_eq!(loaded, Some(54321));
    }

    #[test]
    fn test_port_file_invalid_content_returns_none() {
        let tmp = TempDir::new().unwrap();
        let isolated = tmp.path().join("isolated_app_data");
        std::fs::create_dir_all(&isolated).unwrap();

        let mgr = PostgresManager::new(&isolated);
        std::fs::create_dir_all(&mgr.data_dir).unwrap();

        let port_file = mgr.data_dir.join(PORT_FILE);
        std::fs::write(&port_file, "not-a-number").unwrap();

        assert_eq!(mgr.read_saved_port(), None);
    }

    #[tokio::test]
    async fn test_initialize_without_binaries_returns_unavailable() {
        let tmp = TempDir::new().unwrap();
        let isolated = tmp.path().join("isolated_app_data");
        std::fs::create_dir_all(&isolated).unwrap();

        let mut mgr = PostgresManager::new(&isolated);
        mgr.pg_ctl = None;
        mgr.initdb = None;
        mgr.psql = None;
        mgr.pg_dump = None;

        let result = mgr.initialize().await.unwrap();
        assert!(!result.available);
        assert!(!result.managed);
        assert!(result.port.is_none());
    }

    #[test]
    fn test_connection_string_format() {
        let tmp = TempDir::new().unwrap();
        let mgr = PostgresManager {
            data_dir: tmp.path().to_path_buf(),
            port: 15432,
            username: "testuser".to_string(),
            database: "testdb".to_string(),
            password: Some("secret123".to_string()),
            pg_ctl: None,
            initdb: None,
            psql: None,
            pg_dump: None,
            diagnostics: Vec::new(),
            server_running: false,
            active_external: None,
        };

        let conn_str = mgr.connection_string();
        assert!(conn_str.contains("host=127.0.0.1"));
        assert!(conn_str.contains("port=15432"));
        assert!(conn_str.contains("user=testuser"));
        assert!(conn_str.contains("dbname=testdb"));
        assert!(conn_str.contains("password=secret123"));
    }

    #[test]
    fn test_credential_persistence() {
        let tmp = TempDir::new().unwrap();
        let isolated = tmp.path().join("isolated_app_data");
        std::fs::create_dir_all(&isolated).unwrap();

        let mut mgr = PostgresManager::new(&isolated);
        std::fs::create_dir_all(&mgr.data_dir).unwrap();
        mgr.username = "imagedb".to_string();
        mgr.password = Some("testpassword42".to_string());
        std::fs::write(
            mgr.data_dir.join(CREDENTIAL_FILE),
            format!("{}:{}", mgr.username, mgr.password.as_deref().unwrap_or("")),
        )
        .unwrap();

        let (user, pass) = mgr.read_saved_credentials().unwrap();
        assert_eq!(user, "imagedb");
        assert_eq!(pass, "testpassword42");
    }

    #[test]
    fn test_generate_password_length() {
        let pw = PostgresManager::generate_password();
        assert_eq!(pw.len(), 24);
        assert!(pw.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    /// Real PostgreSQL + pgvector integration test.
    ///
    /// Runs only when `IMAGEDB_POSTGRES_BIN` is set to a directory containing
    /// pg_ctl, initdb, and psql. Without the env var, the test returns
    /// immediately (and still passes) so `cargo test` remains green on
    /// machines that do not have a PostgreSQL binary available.
    ///
    /// Invocation:
    ///   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test \
    ///       --manifest-path apps/desktop/src-tauri/Cargo.toml \
    ///       real_pgvector_full_lifecycle -- --ignored --test-threads=1
    #[tokio::test]
    #[ignore]
    async fn real_pgvector_full_lifecycle() {
        use crate::infrastructure::postgres::MigrationRunner;

        let _bin_dir = match std::env::var("IMAGEDB_POSTGRES_BIN") {
            Ok(v) if !v.is_empty() => v,
            _ => {
                panic!(
                    "IMAGEDB_POSTGRES_BIN is not set; cannot run the real PostgreSQL lifecycle test. \
                     Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run \
                     `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime \
                     at .local/db-tools/postgresql-18.4/pgsql/bin."
                );
            }
        };

        let run = async {
            let tmp = TempDir::new().expect("create tempfile dir");
            let app_data = tmp.path().join("app_data");
            std::fs::create_dir_all(&app_data).expect("create app_data dir");

            let mut mgr = PostgresManager::new(&app_data);
            assert!(
                mgr.binaries_available(),
                "binaries should be found via IMAGEDB_POSTGRES_BIN; diagnostics: {:?}",
                mgr.diagnostics()
            );

            let result = mgr.initialize().await.expect("initialize #1");
            assert!(result.available);
            assert!(result.managed);
            assert!(result.connection_ok);
            assert!(result.pgvector_available);

            let (client, handle) = mgr.connect().await.expect("connect #1");
            let mut client = client;
            let applied = MigrationRunner::run_pending(&mut client)
                .await
                .expect("run_pending #1");
            assert!(!applied.is_empty());

            let version = MigrationRunner::current_version(&client)
                .await
                .expect("current_version #1");
            // Migration 0010 is the current head of the embedded migration chain.
            assert_eq!(version.as_deref(), Some("0010_library_root_leases"));

            drop(client);
            handle.abort();
            mgr.shutdown().await.expect("shutdown #1");

            let mut mgr2 = PostgresManager::new(&app_data);
            assert!(mgr2.binaries_available());

            let result2 = mgr2.initialize().await.expect("initialize #2");
            assert!(result2.managed);
            assert!(result2.connection_ok);
            assert!(result2.pgvector_available);

            let (client2, handle2) = mgr2.connect().await.expect("connect #2");
            let mut client2 = client2;
            let newly_applied = MigrationRunner::run_pending(&mut client2)
                .await
                .expect("run_pending #2");
            assert!(newly_applied.is_empty(), "unexpected: {newly_applied:?}");

            let version2 = MigrationRunner::current_version(&client2)
                .await
                .expect("current_version #2");
            // Migration 0010 is the current head of the embedded migration chain.
            assert_eq!(version2.as_deref(), Some("0010_library_root_leases"));

            drop(client2);
            handle2.abort();
            mgr2.shutdown().await.expect("shutdown #2");
        };

        match tokio::time::timeout(std::time::Duration::from_secs(120), run).await {
            Ok(()) => {}
            Err(_) => panic!("real_pgvector_full_lifecycle timed out after 120s"),
        }
    }

    /// Clean Windows bootstrap: simulate a fresh install that has NO system
    /// PostgreSQL and relies solely on the packaged `postgres-runtime/` the
    /// installer ships. Sets `IMAGEDB_POSTGRES_RUNTIME_DIR` to the repo's
    /// packaged runtime (built by `scripts/package-postgres-runtime.mjs`)
    /// and unsets `IMAGEDB_POSTGRES_BIN`, then runs the full initdb → start →
    /// pgvector → migration lifecycle using only those binaries.
    ///
    /// This is the acceptance test for the M6.5 closure item: "clean Windows
    /// environment, no system PostgreSQL → initialize managed database →
    /// CREATE EXTENSION vector → migration succeeds".
    #[tokio::test]
    #[ignore]
    async fn real_packaged_runtime_clean_bootstrap() {
        use crate::infrastructure::postgres::MigrationRunner;

        // Locate the packaged runtime the installer would ship.
        let packaged_runtime = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join("windows-x86_64")
            .join("postgres-runtime");
        if !packaged_runtime.join("bin").join("postgres.exe").is_file() {
            panic!(
                "Packaged postgres-runtime not found at {} (missing bin/postgres.exe). \
                 Run `node scripts/package-postgres-runtime.mjs` to build it before running the \
                 clean-bootstrap test.",
                packaged_runtime.display()
            );
        }

        // SAFETY: this is an #[ignore] real-db test run with
        // --test-threads=1, so the env var does not race with other tests.
        std::env::set_var("IMAGEDB_POSTGRES_RUNTIME_DIR", &packaged_runtime);
        let prev_bin = std::env::var("IMAGEDB_POSTGRES_BIN").ok();
        std::env::remove_var("IMAGEDB_POSTGRES_BIN");

        let run = async {
            let tmp = TempDir::new().expect("create tempfile dir");
            let app_data = tmp.path().join("app_data");
            std::fs::create_dir_all(&app_data).expect("create app_data dir");

            let mut mgr = PostgresManager::new(&app_data);
            assert!(
                mgr.binaries_available(),
                "locator should find the packaged runtime via IMAGEDB_POSTGRES_RUNTIME_DIR; diagnostics: {:?}",
                mgr.diagnostics()
            );
            assert!(
                mgr.diagnostics()
                    .iter()
                    .any(|d| d.contains("IMAGEDB_POSTGRES_RUNTIME_DIR")),
                "expected the locator to credit the runtime dir, got: {:?}",
                mgr.diagnostics()
            );

            let result = mgr.initialize().await.expect("initialize");
            assert!(
                result.available,
                "managed PG should be available: {result:?}"
            );
            assert!(result.managed);
            assert!(result.connection_ok);
            assert!(
                result.pgvector_available,
                "CREATE EXTENSION vector must succeed with the packaged runtime"
            );

            let (client, handle) = mgr.connect().await.expect("connect");
            let mut client = client;
            let applied = MigrationRunner::run_pending(&mut client)
                .await
                .expect("run_pending");
            assert!(
                !applied.is_empty(),
                "migrations should apply on a fresh bootstrap"
            );
            let version = MigrationRunner::current_version(&client)
                .await
                .expect("current_version");
            assert_eq!(
                version.as_deref(),
                Some("0010_library_root_leases"),
                "migration head must be reached on a clean bootstrap"
            );

            drop(client);
            handle.abort();
            mgr.shutdown().await.expect("shutdown");
        };

        match tokio::time::timeout(std::time::Duration::from_secs(120), run).await {
            Ok(()) => {}
            Err(_) => panic!("real_packaged_runtime_clean_bootstrap timed out after 120s"),
        }

        // Restore env after the run so other real suites in the same
        // process are unaffected. (This test runs under --test-threads=1.)
        std::env::remove_var("IMAGEDB_POSTGRES_RUNTIME_DIR");
        if let Some(prev) = prev_bin {
            std::env::set_var("IMAGEDB_POSTGRES_BIN", prev);
        }
    }
}

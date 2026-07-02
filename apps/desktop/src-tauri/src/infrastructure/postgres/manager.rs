use crate::error::AppError;
use rand::Rng;
use serde::Serialize;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

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
    diagnostics: Vec<String>,
    server_running: bool,
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
            diagnostics: Vec::new(),
            server_running: false,
        };
        mgr.locate_binaries();
        mgr.load_saved_config();
        mgr
    }

    fn locate_binaries(&mut self) {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));

        let search_paths: Vec<PathBuf> = vec![
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

        let exe_suffix = if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        };

        for base in &search_paths {
            let pg_ctl = base.join(format!("pg_ctl{exe_suffix}"));
            let initdb = base.join(format!("initdb{exe_suffix}"));
            let psql = base.join(format!("psql{exe_suffix}"));
            if pg_ctl.exists() && initdb.exists() && psql.exists() {
                self.pg_ctl = Some(pg_ctl);
                self.initdb = Some(initdb);
                self.psql = Some(psql);
                self.diagnostics
                    .push(format!("Found PostgreSQL binaries at: {}", base.display()));
                return;
            }
        }

        for name in ["pg_ctl", "initdb", "psql"] {
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
                            _ => {}
                        }
                    }
                }
            }
        }

        if self.pg_ctl.is_some() && self.initdb.is_some() && self.psql.is_some() {
            self.diagnostics
                .push("Found PostgreSQL binaries via PATH".to_string());
        } else {
            self.diagnostics.push(format!(
                "PostgreSQL binaries not found. Searched: {}, /usr/local/bin, /usr/bin, and PATH. \
                 Install PostgreSQL or place binaries alongside the application executable.",
                exe_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<unknown exe dir>".to_string())
            ));
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

    pub fn database(&self) -> &str {
        &self.database
    }

    pub fn is_server_running(&self) -> bool {
        self.server_running
    }

    pub fn connection_string(&self) -> String {
        let mut parts = vec![
            format!("host=127.0.0.1"),
            format!("port={}", self.port),
            format!("user={}", self.username),
            format!("dbname={}", self.database),
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

    fn save_credentials(&self) -> Result<(), AppError> {
        let path = self.credential_file_path();
        let password = self.password.as_deref().unwrap_or("");
        let content = format!("{}:{}", self.username, password);
        std::fs::write(&path, content).map_err(|e| {
            AppError::Internal(format!(
                "cannot write credential file {}: {e}",
                path.display()
            ))
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(
                |e| AppError::Internal(format!("cannot set credential file permissions: {e}")),
            )?;
        }

        Ok(())
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

    fn psql_command(&self, psql: &std::path::Path) -> Command {
        let mut cmd = Command::new(psql);
        if let Some(ref pw) = self.password {
            cmd.env("PGPASSWORD", pw);
        }
        cmd
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
            std::fs::create_dir_all(&self.data_dir).map_err(|e| {
                AppError::Internal(format!(
                    "cannot create data directory {}: {e}",
                    self.data_dir.display()
                ))
            })?;
            self.diagnostics.push(format!(
                "Created data directory: {}",
                self.data_dir.display()
            ));

            self.port = Self::find_free_port()?;
            self.password = Some(Self::generate_password());
            self.save_port()?;
            self.save_credentials()?;

            let Some(initdb) = self.initdb.as_ref() else {
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
            let pwfile = self.data_dir.join("initdb_pwfile");
            if let Err(e) = std::fs::write(&pwfile, password.as_bytes()) {
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
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await?;

            let _remove_pw = std::fs::remove_file(&pwfile);

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
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

        let listen_opts = format!("-p {port_str} -h 127.0.0.1");

        let output = Command::new(pg_ctl)
            .args([
                "start",
                "-D",
                &data_dir_str,
                "-o",
                &listen_opts,
                "-w",
                "-t",
                "10",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if stderr.contains("already running") || stderr.contains("already started") {
                self.diagnostics
                    .push("PostgreSQL server already running".to_string());
                self.server_running = true;
            } else {
                self.diagnostics
                    .push(format!("pg_ctl start failed: {stderr}"));
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
        let psql = match self.psql.as_ref() {
            Some(p) => p,
            None => {
                self.diagnostics
                    .push("psql binary not available".to_string());
                return false;
            }
        };
        let port_str = self.port.to_string();

        let check = self
            .psql_command(psql)
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                &port_str,
                "-U",
                &self.username,
                "-d",
                "postgres",
                "-tc",
                "SELECT 1 FROM pg_database WHERE datname='imagedb'",
            ])
            .output()
            .await;

        match check {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.trim() == "1" {
                    self.diagnostics
                        .push("Database 'imagedb' already exists".to_string());
                    return true;
                }
            }
            _ => {}
        }

        let create = self
            .psql_command(psql)
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                &port_str,
                "-U",
                &self.username,
                "-d",
                "postgres",
                "-c",
                &format!("CREATE DATABASE {}", self.database),
            ])
            .output()
            .await;

        match create {
            Ok(output) if output.status.success() => {
                self.diagnostics
                    .push(format!("Created database '{}'", self.database));
                true
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                self.diagnostics
                    .push(format!("CREATE DATABASE failed: {stderr}"));
                false
            }
            Err(e) => {
                self.diagnostics.push(format!("psql command failed: {e}"));
                false
            }
        }
    }

    pub async fn check_pgvector(&mut self) -> bool {
        let psql = match self.psql.as_ref() {
            Some(p) => p,
            None => {
                self.diagnostics
                    .push("psql binary not available".to_string());
                return false;
            }
        };
        let port_str = self.port.to_string();

        let output = self
            .psql_command(psql)
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                &port_str,
                "-U",
                &self.username,
                "-d",
                &self.database,
                "-c",
                "CREATE EXTENSION IF NOT EXISTS vector",
            ])
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => {
                self.diagnostics
                    .push("pgvector extension enabled".to_string());
                true
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                self.diagnostics
                    .push(format!("pgvector not available: {stderr}"));
                false
            }
            Err(e) => {
                self.diagnostics.push(format!("pgvector check failed: {e}"));
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
            if self.data_dir.exists() && self.server_running {
                let data_dir_str = Self::path_to_str(&self.data_dir)?;
                let output = Command::new(pg_ctl)
                    .args(["stop", "-D", &data_dir_str, "-m", "fast", "-w"])
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

        assert!(!mgr.binaries_available());
        assert!(
            mgr.diagnostics.iter().any(|d| d.contains("not found")),
            "Expected diagnostic about missing binaries, got: {:?}",
            mgr.diagnostics
        );
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
            diagnostics: Vec::new(),
            server_running: false,
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
        mgr.save_credentials().unwrap();

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
}

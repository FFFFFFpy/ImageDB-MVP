use crate::error::AppError;
use serde::Serialize;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

const PORT_FILE: &str = "postgres_port";

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
    pg_ctl: Option<PathBuf>,
    initdb: Option<PathBuf>,
    psql: Option<PathBuf>,
    diagnostics: Vec<String>,
}

#[allow(dead_code)]
impl PostgresManager {
    pub fn new(app_data_dir: &std::path::Path) -> Self {
        let data_dir = app_data_dir.join("postgres_data");
        let mut mgr = Self {
            data_dir,
            port: 0,
            pg_ctl: None,
            initdb: None,
            psql: None,
            diagnostics: Vec::new(),
        };
        mgr.locate_binaries();
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

    pub fn binaries_available(&self) -> bool {
        self.pg_ctl.is_some() && self.initdb.is_some() && self.psql.is_some()
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

    fn port_file_path(&self) -> PathBuf {
        self.data_dir.join(PORT_FILE)
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

    fn path_to_str(path: &std::path::Path) -> Result<String, AppError> {
        path.to_str().map(|s| s.to_string()).ok_or_else(|| {
            AppError::Internal(format!(
                "path contains non-UTF8 characters: {}",
                path.display()
            ))
        })
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
            self.port = Self::find_free_port()?;
            self.save_port()?;

            std::fs::create_dir_all(&self.data_dir)?;
            self.diagnostics.push(format!(
                "Created data directory: {}",
                self.data_dir.display()
            ));

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
            let data_dir_str = Self::path_to_str(&self.data_dir)?;
            let output = Command::new(initdb)
                .args([
                    "-D",
                    &data_dir_str,
                    "--no-locale",
                    "--encoding=UTF8",
                    "--auth=trust",
                    "--username=imagedb",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await?;

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

    async fn start_server(&mut self) -> Result<PostgresProbeResult, AppError> {
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

        self.diagnostics
            .push(format!("PostgreSQL started on port {port_str}"));

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

        let check = Command::new(psql)
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                &port_str,
                "-U",
                "imagedb",
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

        let create = Command::new(psql)
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                &port_str,
                "-U",
                "imagedb",
                "-d",
                "postgres",
                "-c",
                "CREATE DATABASE imagedb",
            ])
            .output()
            .await;

        match create {
            Ok(output) if output.status.success() => {
                self.diagnostics
                    .push("Created database 'imagedb'".to_string());
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

    async fn check_pgvector(&mut self) -> bool {
        let psql = match self.psql.as_ref() {
            Some(p) => p,
            None => {
                self.diagnostics
                    .push("psql binary not available".to_string());
                return false;
            }
        };
        let port_str = self.port.to_string();

        let output = Command::new(psql)
            .args([
                "-h",
                "127.0.0.1",
                "-p",
                &port_str,
                "-U",
                "imagedb",
                "-d",
                "imagedb",
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

    async fn test_connection(&mut self) -> bool {
        let conn_str = format!(
            "host=127.0.0.1 port={} user=imagedb dbname=imagedb",
            self.port
        );

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

    pub async fn shutdown(&mut self) -> Result<(), AppError> {
        if let Some(pg_ctl) = &self.pg_ctl {
            if self.data_dir.exists() {
                let data_dir_str = Self::path_to_str(&self.data_dir)?;
                let output = Command::new(pg_ctl)
                    .args(["stop", "-D", &data_dir_str, "-m", "fast", "-w"])
                    .output()
                    .await?;

                if output.status.success() {
                    self.diagnostics.push("PostgreSQL stopped".to_string());
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    self.diagnostics
                        .push(format!("pg_ctl stop warning: {stderr}"));
                }
            }
        }
        Ok(())
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
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
}

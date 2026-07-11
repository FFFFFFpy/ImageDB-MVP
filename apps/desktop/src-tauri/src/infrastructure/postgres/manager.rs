use crate::domain::ConnectionConfig;
use crate::error::AppError;
use crate::infrastructure::postgres::connect_external;
use crate::infrastructure::postgres::MigrationRunner;
use rand::Rng;
use serde::Serialize;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::process::Command;
use tokio::time::{timeout, Duration};

const PORT_FILE: &str = "postgres_port";
const CREDENTIAL_FILE: &str = "postgres_credentials";
const INITDB_COMPLETE_FILE: &str = "imagedb_initdb_complete";
const DEFAULT_USERNAME: &str = "imagedb";
const DEFAULT_DATABASE: &str = "imagedb";
const PG_CTL_START_TIMEOUT_SECS: &str = "45";
const PG_CTL_STATUS_TIMEOUT_SECS: u64 = 10;
const PG_CTL_NOT_RUNNING_EXIT_CODE: i32 = 3;

fn validate_shutdown_status_after_failed_stop(
    status_code: Option<i32>,
    stop_stderr: &str,
    status_stdout: &str,
    status_stderr: &str,
) -> Result<(), String> {
    match status_code {
        Some(PG_CTL_NOT_RUNNING_EXIT_CODE) => Ok(()),
        Some(0) => Err(format!(
            "pg_ctl stop returned non-zero and pg_ctl status reports the server is still running; stop stderr: {stop_stderr}; status stdout: {status_stdout}; status stderr: {status_stderr}"
        )),
        Some(code) => Err(format!(
            "pg_ctl stop returned non-zero and pg_ctl status returned unexpected exit code {code}; stop stderr: {stop_stderr}; status stdout: {status_stdout}; status stderr: {status_stderr}"
        )),
        None => Err(format!(
            "pg_ctl stop returned non-zero and pg_ctl status terminated without an exit code; stop stderr: {stop_stderr}; status stdout: {status_stdout}; status stderr: {status_stderr}"
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeLookup {
    Found,
    ContinueSearch,
    RequiredRuntimeMissing,
}

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
    schema_ready: AtomicBool,
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
            schema_ready: AtomicBool::new(false),
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

        let runtime_required = std::env::var("IMAGEDB_POSTGRES_RUNTIME_REQUIRED")
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                )
            })
            .unwrap_or(false);
        let runtime_dir = std::env::var_os("IMAGEDB_POSTGRES_RUNTIME_DIR").map(PathBuf::from);
        match self.locate_configured_runtime(runtime_dir.as_deref(), runtime_required, exe_suffix) {
            RuntimeLookup::Found | RuntimeLookup::RequiredRuntimeMissing => return,
            RuntimeLookup::ContinueSearch => {}
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

    fn locate_configured_runtime(
        &mut self,
        runtime_dir: Option<&std::path::Path>,
        required: bool,
        exe_suffix: &str,
    ) -> RuntimeLookup {
        let Some(runtime) = runtime_dir else {
            if required {
                self.diagnostics.push(
                    "The bundled PostgreSQL runtime path is unavailable. The ImageDB install is \
                     incomplete; reinstall ImageDB. System PostgreSQL fallback is disabled for \
                     release builds."
                        .to_string(),
                );
                return RuntimeLookup::RequiredRuntimeMissing;
            }
            return RuntimeLookup::ContinueSearch;
        };

        let bin_dir = runtime.join("bin");
        let found = if required {
            self.try_use_complete_packaged_runtime(&bin_dir, exe_suffix)
        } else {
            self.try_use_bin_dir(&bin_dir, exe_suffix)
        };
        if found {
            self.diagnostics.push(format!(
                "Found bundled PostgreSQL runtime via IMAGEDB_POSTGRES_RUNTIME_DIR: {}",
                runtime.display()
            ));
            return RuntimeLookup::Found;
        }

        if required {
            self.diagnostics.push(format!(
                "Bundled PostgreSQL runtime '{}' is incomplete (expected postgres, pg_ctl, \
                 initdb, psql, and pg_dump in bin). Reinstall ImageDB; system PostgreSQL \
                 fallback is disabled for release builds.",
                runtime.display()
            ));
            RuntimeLookup::RequiredRuntimeMissing
        } else {
            self.diagnostics.push(format!(
                "IMAGEDB_POSTGRES_RUNTIME_DIR='{}' is set but missing bin/pg_ctl/initdb/psql; \
                 falling back to default search",
                runtime.display()
            ));
            RuntimeLookup::ContinueSearch
        }
    }

    fn try_use_complete_packaged_runtime(
        &mut self,
        base: &std::path::Path,
        exe_suffix: &str,
    ) -> bool {
        let postgres = strip_verbatim_prefix(&base.join(format!("postgres{exe_suffix}")));
        let pg_dump = strip_verbatim_prefix(&base.join(format!("pg_dump{exe_suffix}")));
        postgres.exists() && pg_dump.exists() && self.try_use_bin_dir(base, exe_suffix)
    }

    pub(crate) fn try_use_bin_dir(&mut self, base: &std::path::Path, exe_suffix: &str) -> bool {
        let pg_ctl = strip_verbatim_prefix(&base.join(format!("pg_ctl{exe_suffix}")));
        let initdb = strip_verbatim_prefix(&base.join(format!("initdb{exe_suffix}")));
        let psql = strip_verbatim_prefix(&base.join(format!("psql{exe_suffix}")));
        let pg_dump = strip_verbatim_prefix(&base.join(format!("pg_dump{exe_suffix}")));
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
        let same_database = self
            .active_external
            .as_ref()
            .map(|current| {
                current.host == config.host
                    && current.port == config.port
                    && current.database == config.database
            })
            .unwrap_or(false);
        if !same_database {
            self.schema_ready.store(false, Ordering::Release);
        }
        self.active_external = Some(config);
    }

    pub fn use_managed_profile(&mut self) {
        if self.active_external.is_some() {
            self.schema_ready.store(false, Ordering::Release);
        }
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

    pub fn cluster_files_exist(&self) -> bool {
        self.data_dir.join("PG_VERSION").is_file()
            && self.data_dir.join("base").is_dir()
            && self.data_dir.join("global").is_dir()
            && self.data_dir.join("global").join("pg_control").is_file()
            && self.data_dir.join("pg_wal").is_dir()
            && self.data_dir.join("postgresql.conf").is_file()
            && self.data_dir.join("pg_hba.conf").is_file()
    }

    pub async fn initialize(&mut self) -> Result<PostgresProbeResult, AppError> {
        self.schema_ready.store(false, Ordering::Release);
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

        let parent_dir = self
            .data_dir
            .parent()
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "data directory {} has no parent",
                    self.data_dir.display()
                ))
            })?
            .to_path_buf();
        let staging_dir = parent_dir.join("postgres_staging");
        let has_initialization_marker =
            staging_dir.join(PORT_FILE).is_file() && staging_dir.join(CREDENTIAL_FILE).is_file();
        let initdb_completed = staging_dir.join(INITDB_COMPLETE_FILE).is_file();
        let has_published_metadata = self.data_dir.join(PORT_FILE).is_file()
            || self.data_dir.join(CREDENTIAL_FILE).is_file();

        if self.data_dir.exists()
            && has_initialization_marker
            && !initdb_completed
            && !has_published_metadata
        {
            // Both staged metadata files are written before initdb starts
            // and the completion marker is written only after initdb exits
            // successfully and the cluster structure is verified. Without
            // that post-init marker, even a directory that superficially
            // resembles a cluster is safe to discard and retry.
            std::fs::remove_dir_all(&self.data_dir).map_err(|e| {
                AppError::Internal(format!(
                    "cannot clean interrupted PostgreSQL cluster {}: {e}",
                    self.data_dir.display()
                ))
            })?;
            std::fs::remove_dir_all(&staging_dir).map_err(|e| {
                AppError::Internal(format!(
                    "cannot clean interrupted PostgreSQL staging directory {}: {e}",
                    staging_dir.display()
                ))
            })?;
            self.diagnostics.push(
                "Recovered an interrupted managed PostgreSQL initialization; retrying initdb"
                    .to_string(),
            );
        }

        if self.data_dir.exists() && !self.cluster_files_exist() {
            self.diagnostics.push(format!(
                    "Refusing to reuse or delete incomplete PostgreSQL data directory {} because no safely recoverable ImageDB initialization is present",
                    self.data_dir.display()
                ));
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

        if self.cluster_files_exist() && staging_dir.exists() {
            if !initdb_completed {
                self.diagnostics.push(format!(
                    "Refusing to publish PostgreSQL metadata from unrecognized staging directory {} because the post-initdb completion marker is missing",
                    staging_dir.display()
                ));
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
            // initdb completed, but the process died before publishing its
            // generated connection metadata. Finish that small atomic handoff
            // instead of generating credentials that no longer match the DB.
            for file_name in [PORT_FILE, CREDENTIAL_FILE] {
                let staged = staging_dir.join(file_name);
                let published = self.data_dir.join(file_name);
                if !published.is_file() {
                    if !staged.is_file() {
                        self.diagnostics.push(format!(
                            "Managed PostgreSQL initialization metadata is incomplete: neither {} nor {} exists",
                            published.display(),
                            staged.display()
                        ));
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
                    std::fs::rename(&staged, &published).map_err(|e| {
                        AppError::Internal(format!(
                            "cannot finish managed PostgreSQL metadata publish {} -> {}: {e}",
                            staged.display(),
                            published.display()
                        ))
                    })?;
                }
            }
            let _ = std::fs::remove_dir_all(&staging_dir);
            self.load_saved_config();
            self.diagnostics.push(
                "Recovered managed PostgreSQL metadata after interrupted initialization"
                    .to_string(),
            );
        }

        if !self.data_dir.exists() {
            std::fs::create_dir_all(&parent_dir).map_err(|e| {
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

            std::fs::write(staging_dir.join(INITDB_COMPLETE_FILE), b"initdb-complete\n").map_err(
                |e| {
                    AppError::Internal(format!(
                        "cannot persist initdb completion marker in {}: {e}",
                        staging_dir.display()
                    ))
                },
            )?;

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
                PG_CTL_START_TIMEOUT_SECS,
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

    pub async fn connect_raw(
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

    /// Connect to the active application database and ensure its embedded
    /// schema is current before returning the client to business code.
    ///
    /// Raw/preflight/migration-copy paths must use `connect_raw`; normal app
    /// commands use this method so an already configured database is upgraded
    /// automatically after installing a newer ImageDB build.
    pub async fn connect(
        &self,
    ) -> Result<(tokio_postgres::Client, tokio::task::JoinHandle<()>), AppError> {
        let (mut client, handle) = self.connect_raw().await?;
        if self.schema_ready.load(Ordering::Acquire) {
            return Ok((client, handle));
        }

        if let Err(error) = MigrationRunner::run_pending(&mut client).await {
            handle.abort();
            return Err(AppError::Internal(format!(
                "failed to prepare active ImageDB schema: {error}"
            )));
        }
        self.schema_ready.store(true, Ordering::Release);
        Ok((client, handle))
    }

    pub async fn shutdown(&mut self) -> Result<(), AppError> {
        if let Some(pg_ctl) = &self.pg_ctl {
            let has_postmaster = self.data_dir.join("postmaster.pid").exists();
            if self.data_dir.exists() && (self.server_running || has_postmaster) {
                let data_dir_str = Self::path_to_str(&self.data_dir)?;
                let mut stop_command = Command::new(pg_ctl);
                stop_command
                    .args(["stop", "-D", &data_dir_str, "-m", "fast", "-w"])
                    .stdin(Stdio::null())
                    .kill_on_drop(true);
                let output = stop_command.output().await?;

                if output.status.success() {
                    self.diagnostics.push("PostgreSQL stopped".to_string());
                    self.server_running = false;
                } else {
                    let stop_stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    let mut status_command = Command::new(pg_ctl);
                    status_command
                        .args(["status", "-D", &data_dir_str])
                        .stdin(Stdio::null())
                        .kill_on_drop(true);
                    let status_output = match timeout(
                        Duration::from_secs(PG_CTL_STATUS_TIMEOUT_SECS),
                        status_command.output(),
                    )
                    .await
                    {
                        Ok(Ok(output)) => output,
                        Ok(Err(error)) => {
                            let message = format!(
                                "pg_ctl stop returned non-zero and the follow-up status command failed: {error}; stop stderr: {stop_stderr}"
                            );
                            self.diagnostics.push(message.clone());
                            return Err(AppError::PostgresUnavailable(message));
                        }
                        Err(_) => {
                            let message = format!(
                                "pg_ctl stop returned non-zero and follow-up status timed out after {PG_CTL_STATUS_TIMEOUT_SECS}s; stop stderr: {stop_stderr}"
                            );
                            self.diagnostics.push(message.clone());
                            return Err(AppError::PostgresUnavailable(message));
                        }
                    };
                    let status_stdout = String::from_utf8_lossy(&status_output.stdout)
                        .trim()
                        .to_string();
                    let status_stderr = String::from_utf8_lossy(&status_output.stderr)
                        .trim()
                        .to_string();
                    if let Err(message) = validate_shutdown_status_after_failed_stop(
                        status_output.status.code(),
                        &stop_stderr,
                        &status_stdout,
                        &status_stderr,
                    ) {
                        self.diagnostics.push(message.clone());
                        return Err(AppError::PostgresUnavailable(message));
                    }

                    self.diagnostics.push(format!(
                        "pg_ctl stop returned non-zero, but follow-up status confirmed PostgreSQL is stopped: {stop_stderr}"
                    ));
                    self.server_running = false;
                }
            }
        }
        Ok(())
    }
}

/// Strip the Windows verbatim path prefix `\\?\` from a path's string form.
///
/// Tauri's `resource_dir()` returns a `\\?\`-prefixed verbatim path on
/// Windows. PostgreSQL's `initdb` (and `pg_ctl`) derive the path to the
/// sibling `postgres.exe` from their own executable path; when that path
/// is verbatim (`\\?\D:\…\bin\initdb.exe`), initdb's internal
/// normalization mangles it into `//?/D:/…`, Windows rejects it, and
/// initdb fails with 'initdb 需要程序 "postgres" … 找不到该程序' even though
/// postgres.exe is right there. Stripping the prefix before we store the
/// binary paths makes every spawned child (initdb, pg_ctl, psql) receive a
/// standard path that round-trips correctly.
fn strip_verbatim_prefix(path: &std::path::Path) -> PathBuf {
    let s = path.to_str().unwrap_or_default();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        // A bare `\\?\C:\…` verbatim disk path → `C:\…`.
        if rest.len() >= 2 && rest.as_bytes()[1] == b':' {
            return PathBuf::from(rest);
        }
        // `\\?\UNC\server\share` → `\\server\share`.
        if let Some(unc_rest) = rest.strip_prefix("UNC\\") {
            return PathBuf::from(format!(r"\\{}", unc_rest));
        }
        // Anything else: return the stripped form as-is.
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn shutdown_failed_stop_accepts_status_not_running() {
        assert!(validate_shutdown_status_after_failed_stop(
            Some(PG_CTL_NOT_RUNNING_EXIT_CODE),
            "stop command reported an error",
            "no server running",
            "",
        )
        .is_ok());
    }

    #[test]
    fn shutdown_failed_stop_rejects_status_still_running() {
        let error = validate_shutdown_status_after_failed_stop(
            Some(0),
            "stop command reported an error",
            "server is running",
            "",
        )
        .expect_err("status exit code 0 must keep graceful shutdown from exiting");
        assert!(error.contains("still running"));
    }

    #[test]
    fn shutdown_failed_stop_rejects_abnormal_status() {
        let unexpected = validate_shutdown_status_after_failed_stop(
            Some(4),
            "stop command reported an error",
            "",
            "invalid data directory",
        )
        .expect_err("an abnormal status code must fail shutdown");
        assert!(unexpected.contains("unexpected exit code 4"));

        let terminated = validate_shutdown_status_after_failed_stop(
            None,
            "stop command reported an error",
            "",
            "terminated",
        )
        .expect_err("status without an exit code must fail shutdown");
        assert!(terminated.contains("without an exit code"));
    }

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
        // <resources>/postgres-runtime/bin/{postgres,pg_ctl,initdb,psql,pg_dump}
        let bin_dir = tmp.path().join("postgres-runtime").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let exe_suffix = if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        };
        for name in ["postgres", "pg_ctl", "initdb", "psql", "pg_dump"] {
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

    #[test]
    fn test_release_runtime_policy_rejects_incomplete_bundle_without_fallback() {
        let tmp = TempDir::new().unwrap();
        let isolated = tmp.path().join("isolated_app_data");
        std::fs::create_dir_all(&isolated).unwrap();
        let runtime_dir = tmp.path().join("postgres-runtime");
        let bin_dir = runtime_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let exe_suffix = if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        };
        for name in ["pg_ctl", "initdb", "psql"] {
            std::fs::write(bin_dir.join(format!("{name}{exe_suffix}")), b"").unwrap();
        }

        let mut mgr = PostgresManager::new(&isolated);
        mgr.pg_ctl = None;
        mgr.initdb = None;
        mgr.psql = None;
        mgr.pg_dump = None;
        mgr.diagnostics.clear();

        assert_eq!(
            mgr.locate_configured_runtime(Some(&runtime_dir), true, exe_suffix),
            RuntimeLookup::RequiredRuntimeMissing
        );
        assert!(!mgr.binaries_available());
        assert!(mgr.diagnostics.iter().any(
            |message| message.contains("fallback is disabled") && message.contains("Reinstall")
        ));
    }

    #[test]
    fn test_release_runtime_policy_accepts_only_complete_bundle() {
        let tmp = TempDir::new().unwrap();
        let isolated = tmp.path().join("isolated_app_data");
        std::fs::create_dir_all(&isolated).unwrap();
        let runtime_dir = tmp.path().join("postgres-runtime");
        let bin_dir = runtime_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let exe_suffix = if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        };
        for name in ["postgres", "pg_ctl", "initdb", "psql", "pg_dump"] {
            std::fs::write(bin_dir.join(format!("{name}{exe_suffix}")), b"").unwrap();
        }

        let mut mgr = PostgresManager::new(&isolated);
        mgr.pg_ctl = None;
        mgr.initdb = None;
        mgr.psql = None;
        mgr.pg_dump = None;
        mgr.diagnostics.clear();

        assert_eq!(
            mgr.locate_configured_runtime(Some(&runtime_dir), true, exe_suffix),
            RuntimeLookup::Found
        );
        assert!(mgr.binaries_available());
        for path in mgr
            .pg_ctl
            .iter()
            .chain(mgr.initdb.iter())
            .chain(mgr.psql.iter())
            .chain(mgr.pg_dump.iter())
        {
            assert!(
                path.starts_with(&bin_dir),
                "unexpected runtime path: {}",
                path.display()
            );
        }
    }

    /// Tauri's `resource_dir()` returns a `\\?\`-prefixed verbatim path on
    /// Windows. PostgreSQL's initdb derives the sibling `postgres.exe` path
    /// from its own executable path and chokes on the verbatim prefix
    /// (mangling `\\?\D:\…` into `//?/D:/…` and failing with "initdb needs
    /// postgres … 找不到该程序"). The probe must strip the prefix before
    /// storing the binary paths so spawned children receive standard paths.
    #[cfg(target_os = "windows")]
    #[test]
    fn test_locator_strips_verbatim_prefix_from_runtime_path() {
        let tmp = TempDir::new().unwrap();
        let isolated = tmp.path().join("isolated_app_data");
        std::fs::create_dir_all(&isolated).unwrap();

        let bin_dir = tmp.path().join("postgres-runtime").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        for name in ["pg_ctl", "initdb", "psql", "pg_dump"] {
            std::fs::write(bin_dir.join(format!("{name}.exe")), b"").unwrap();
        }

        // Synthesize a verbatim path the way Tauri's resource_dir() does.
        let plain = bin_dir.to_str().unwrap();
        let verbatim = PathBuf::from(format!(r"\\?\{plain}"));

        let mut mgr = PostgresManager::new(&isolated);
        mgr.pg_ctl = None;
        mgr.initdb = None;
        mgr.psql = None;
        mgr.pg_dump = None;

        assert!(mgr.try_use_bin_dir(&verbatim, ".exe"));
        assert!(mgr.binaries_available());
        // None of the stored paths may carry the `\\?\` prefix — initdb and
        // pg_ctl would mangle it.
        for p in mgr
            .pg_ctl
            .iter()
            .chain(mgr.initdb.iter())
            .chain(mgr.psql.iter())
            .chain(mgr.pg_dump.iter())
        {
            assert!(
                !p.to_str().unwrap_or_default().starts_with(r"\\?\"),
                "stored binary path still verbatim: {}",
                p.display()
            );
        }
    }

    #[test]
    fn test_strip_verbatim_prefix_disk_path() {
        let p = std::path::Path::new(r"\\?\C:\Program Files\Postgres\bin\initdb.exe");
        assert_eq!(
            strip_verbatim_prefix(p),
            PathBuf::from(r"C:\Program Files\Postgres\bin\initdb.exe")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_strip_verbatim_prefix_unc_path() {
        let p = std::path::Path::new(r"\\?\UNC\server\share\runtime\bin\initdb.exe");
        assert_eq!(
            strip_verbatim_prefix(p),
            PathBuf::from(r"\\server\share\runtime\bin\initdb.exe")
        );
    }

    #[test]
    fn test_strip_verbatim_prefix_plain_path_unchanged() {
        let p = std::path::Path::new(r"C:\Postgres\bin\initdb.exe");
        assert_eq!(
            strip_verbatim_prefix(p),
            PathBuf::from(r"C:\Postgres\bin\initdb.exe")
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
            schema_ready: AtomicBool::new(false),
        };

        let conn_str = mgr.connection_string();
        assert!(conn_str.contains("host=127.0.0.1"));
        assert!(conn_str.contains("port=15432"));
        assert!(conn_str.contains("user=testuser"));
        assert!(conn_str.contains("dbname=testdb"));
        assert!(conn_str.contains("password=secret123"));
    }

    #[test]
    fn schema_readiness_cache_is_invalidated_only_when_database_changes() {
        use crate::domain::TlsMode;

        let tmp = TempDir::new().unwrap();
        let mut mgr = PostgresManager::new(tmp.path());
        mgr.schema_ready.store(true, Ordering::Release);

        let external = ConnectionConfig {
            host: "db.example.test".to_string(),
            port: 5432,
            database: "imagedb".to_string(),
            username: "user-a".to_string(),
            password: Some("secret-a".to_string()),
            tls_mode: TlsMode::Require,
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
            connect_timeout_secs: 10,
            query_timeout_secs: 15,
            profile_name: None,
        };
        mgr.use_external_profile(external.clone());
        assert!(!mgr.schema_ready.load(Ordering::Acquire));

        mgr.schema_ready.store(true, Ordering::Release);
        let mut same_database = external.clone();
        same_database.username = "user-b".to_string();
        same_database.password = Some("secret-b".to_string());
        mgr.use_external_profile(same_database);
        assert!(mgr.schema_ready.load(Ordering::Acquire));

        let mut different_database = external;
        different_database.database = "other".to_string();
        mgr.use_external_profile(different_database);
        assert!(!mgr.schema_ready.load(Ordering::Acquire));

        mgr.schema_ready.store(true, Ordering::Release);
        mgr.use_managed_profile();
        assert!(!mgr.schema_ready.load(Ordering::Acquire));
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
            assert!(
                applied.is_empty(),
                "connect must already prepare the active schema: {applied:?}"
            );

            let version = MigrationRunner::current_version(&client)
                .await
                .expect("current_version #1");
            // The normal connection path must expose the current migration head.
            assert_eq!(version.as_deref(), Some(MigrationRunner::latest_version()));

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
            // Relaunch remains idempotently at the current migration head.
            assert_eq!(version2.as_deref(), Some(MigrationRunner::latest_version()));

            drop(client2);
            handle2.abort();
            mgr2.shutdown().await.expect("shutdown #2");
        };

        match tokio::time::timeout(std::time::Duration::from_secs(120), run).await {
            Ok(()) => {}
            Err(_) => panic!("real_pgvector_full_lifecycle timed out after 120s"),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn real_pgvector_partial_initialization_recovers_safely() {
        let _bin_dir = match std::env::var("IMAGEDB_POSTGRES_BIN") {
            Ok(value) if !value.is_empty() => value,
            _ => panic!("IMAGEDB_POSTGRES_BIN is required for partial-init recovery test"),
        };

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let partial_data = app_data.join("postgres_data");
        let staging = app_data.join("postgres_staging");
        std::fs::create_dir_all(&partial_data).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(partial_data.join("PG_VERSION"), b"18").unwrap();
        std::fs::create_dir_all(partial_data.join("base")).unwrap();
        std::fs::create_dir_all(partial_data.join("global")).unwrap();
        std::fs::create_dir_all(partial_data.join("pg_wal")).unwrap();
        std::fs::write(staging.join(PORT_FILE), b"54321").unwrap();
        std::fs::write(staging.join(CREDENTIAL_FILE), b"imagedb:interrupted").unwrap();

        let mut manager = PostgresManager::new(&app_data);
        let result = manager.initialize().await.unwrap();
        assert!(result.managed, "diagnostics: {:?}", result.diagnostics);
        assert!(
            result.connection_ok,
            "diagnostics: {:?}",
            result.diagnostics
        );
        assert!(manager.cluster_files_exist());
        assert!(!staging.exists());
        assert!(result
            .diagnostics
            .iter()
            .any(|line| line.contains("interrupted managed PostgreSQL initialization")));
        manager.shutdown().await.unwrap();

        // A valid, already-published cluster plus a suspicious unmarked
        // staging directory is not an interrupted first initialization.
        // Preserve the database and fail closed instead of treating the two
        // staged metadata files as authority to delete an established cluster.
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join(PORT_FILE), b"54322").unwrap();
        std::fs::write(staging.join(CREDENTIAL_FILE), b"imagedb:suspicious").unwrap();
        let established_sentinel = partial_data.join("established-data-sentinel.txt");
        std::fs::write(&established_sentinel, b"must survive").unwrap();
        let mut established_manager = PostgresManager::new(&app_data);
        let refused_staging = established_manager.initialize().await.unwrap();
        assert!(!refused_staging.connection_ok);
        assert!(
            established_sentinel.is_file(),
            "published cluster data must survive an unmarked staging conflict"
        );
        assert!(refused_staging
            .diagnostics
            .iter()
            .any(|line| line.contains("completion marker is missing")));

        let unknown_app_data = tmp.path().join("unknown_app_data");
        let unknown_data = unknown_app_data.join("postgres_data");
        let unknown_staging = unknown_app_data.join("postgres_staging");
        std::fs::create_dir_all(&unknown_data).unwrap();
        std::fs::create_dir_all(&unknown_staging).unwrap();
        let sentinel = unknown_data.join("do-not-delete.txt");
        let staging_sentinel = unknown_staging.join("do-not-delete.txt");
        std::fs::write(&sentinel, b"unknown user data").unwrap();
        std::fs::write(&staging_sentinel, b"unknown staging data").unwrap();
        let mut unknown_manager = PostgresManager::new(&unknown_app_data);
        let refused = unknown_manager.initialize().await.unwrap();
        assert!(!refused.connection_ok);
        assert!(sentinel.is_file(), "unknown partial data must be preserved");
        assert!(
            staging_sentinel.is_file(),
            "an unmarked staging directory must be preserved"
        );
        assert!(refused
            .diagnostics
            .iter()
            .any(|line| line.contains("Refusing to reuse or delete incomplete")));
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
                applied.is_empty(),
                "connect must migrate a fresh bootstrap before returning: {applied:?}"
            );
            let version = MigrationRunner::current_version(&client)
                .await
                .expect("current_version");
            assert_eq!(
                version.as_deref(),
                Some(MigrationRunner::latest_version()),
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

pub mod duplicate_group;
pub mod import_state;
pub mod state_machine;

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseMode {
    ManagedLocal,
    External,
}

impl fmt::Display for DatabaseMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ManagedLocal => write!(f, "managed_local"),
            Self::External => write!(f, "external"),
        }
    }
}

impl DatabaseMode {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "managed_local" => Some(Self::ManagedLocal),
            "external" => Some(Self::External),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DatabaseStatus {
    NotInitialized,
    Initializing,
    Ready,
    Connected,
    Error(String),
    BinariesMissing(String),
}

impl DatabaseStatus {
    #[allow(dead_code)]
    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Ready | Self::Connected)
    }
}

impl fmt::Display for DatabaseStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "not_initialized"),
            Self::Initializing => write!(f, "initializing"),
            Self::Ready => write!(f, "ready"),
            Self::Connected => write!(f, "connected"),
            Self::Error(msg) => write!(f, "error: {msg}"),
            Self::BinariesMissing(msg) => write!(f, "binaries_missing: {msg}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    Disable,
    Require,
    VerifyCa,
    #[default]
    VerifyFull,
}

impl TlsMode {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "disable" => Some(Self::Disable),
            "require" => Some(Self::Require),
            "verify_ca" => Some(Self::VerifyCa),
            "verify_full" => Some(Self::VerifyFull),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Disable => "disable",
            Self::Require => "require",
            Self::VerifyCa => "verify_ca",
            Self::VerifyFull => "verify_full",
        }
    }

    pub fn libpq_sslmode(&self) -> &'static str {
        match self {
            Self::Disable => "disable",
            Self::Require => "require",
            Self::VerifyCa => "verify-ca",
            Self::VerifyFull => "verify-full",
        }
    }
}

fn default_external_connect_timeout_secs() -> u64 {
    10
}

fn default_external_query_timeout_secs() -> u64 {
    15
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
    #[serde(default)]
    pub tls_mode: TlsMode,
    #[serde(default)]
    pub ca_cert_path: Option<String>,
    #[serde(default)]
    pub client_cert_path: Option<String>,
    #[serde(default)]
    pub client_key_path: Option<String>,
    #[serde(default = "default_external_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_external_query_timeout_secs")]
    pub query_timeout_secs: u64,
    #[serde(default)]
    pub profile_name: Option<String>,
}

impl ConnectionConfig {
    pub fn connection_string(&self) -> String {
        let mut parts = vec![
            format!("host={}", self.host),
            format!("port={}", self.port),
            format!("dbname={}", self.database),
            format!("user={}", self.username),
        ];
        if let Some(ref pw) = self.password {
            parts.push(format!("password={pw}"));
        }
        parts.push(format!("sslmode={}", self.tls_mode.as_str()));
        parts.push(format!("connect_timeout={}", self.connect_timeout_secs));
        parts.join(" ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedDbConfig {
    pub data_dir: String,
    pub port: u16,
    pub username: String,
    pub database: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalCheckResult {
    pub connection_ok: bool,
    pub version: Option<String>,
    pub version_ok: bool,
    pub tls_mode: TlsMode,
    pub tls_ok: bool,
    pub pgvector_available: bool,
    pub can_create_extension: bool,
    pub can_create_tables: bool,
    pub can_modify_schema: bool,
    pub read_write_ok: bool,
    pub encoding_ok: bool,
    pub timezone_ok: bool,
    pub not_read_only: bool,
    pub migration_state_ok: bool,
    pub schema_compatible: bool,
    pub migration_version: Option<String>,
    pub checks: Vec<ExternalPreflightCheck>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableRowCount {
    pub table: String,
    pub managed_rows: i64,
    pub external_rows: i64,
    pub matches: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalMigrationResult {
    pub switched: bool,
    pub backup_path: Option<String>,
    pub migration_version: Option<String>,
    pub row_counts: Vec<TableRowCount>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalMigrationProgress {
    pub state: String,
    pub current_stage: String,
    pub switched: bool,
    pub backup_path: Option<String>,
    pub migration_version: Option<String>,
    pub row_counts: Vec<TableRowCount>,
    pub diagnostics: Vec<String>,
    pub errors: Vec<String>,
    pub cancel_requested: bool,
}

impl ExternalMigrationProgress {
    pub fn idle() -> Self {
        Self {
            state: "idle".to_string(),
            current_stage: "idle".to_string(),
            switched: false,
            backup_path: None,
            migration_version: None,
            row_counts: Vec::new(),
            diagnostics: Vec::new(),
            errors: Vec::new(),
            cancel_requested: false,
        }
    }

    pub fn running(stage: &str) -> Self {
        Self {
            state: "running".to_string(),
            current_stage: stage.to_string(),
            ..Self::idle()
        }
    }

    pub fn completed(result: &ExternalMigrationResult) -> Self {
        Self {
            state: "completed".to_string(),
            current_stage: if result.switched {
                "switched".to_string()
            } else {
                "not_switched".to_string()
            },
            switched: result.switched,
            backup_path: result.backup_path.clone(),
            migration_version: result.migration_version.clone(),
            row_counts: result.row_counts.clone(),
            diagnostics: result.diagnostics.clone(),
            errors: Vec::new(),
            cancel_requested: false,
        }
    }

    pub fn cancelled(stage: &str, diagnostics: Vec<String>) -> Self {
        Self {
            state: "cancelled".to_string(),
            current_stage: stage.to_string(),
            diagnostics,
            cancel_requested: true,
            errors: vec!["external migration cancelled by user; profile not switched".to_string()],
            ..Self::idle()
        }
    }

    pub fn failed(stage: &str, error: String, diagnostics: Vec<String>) -> Self {
        Self {
            state: "failed".to_string(),
            current_stage: stage.to_string(),
            diagnostics,
            errors: vec![error],
            ..Self::idle()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreflightStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalPreflightCheck {
    pub code: String,
    pub status: PreflightStatus,
    pub message: String,
}

impl ExternalPreflightCheck {
    pub fn pass(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            status: PreflightStatus::Pass,
            message: message.into(),
        }
    }

    pub fn warn(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            status: PreflightStatus::Warn,
            message: message.into(),
        }
    }

    pub fn fail(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            status: PreflightStatus::Fail,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseState {
    pub mode: Option<DatabaseMode>,
    pub status: DatabaseStatus,
    pub managed_config: Option<ManagedDbConfig>,
    pub external_config: Option<ConnectionConfig>,
    pub pgvector_available: bool,
    pub migration_version: Option<String>,
    pub diagnostics: Vec<String>,
}

impl DatabaseState {
    #[allow(dead_code)]
    pub fn not_initialized() -> Self {
        Self {
            mode: None,
            status: DatabaseStatus::NotInitialized,
            managed_config: None,
            external_config: None,
            pgvector_available: false,
            migration_version: None,
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransactionState {
    Planned,
    Staging,
    Verifying,
    Verified,
    Publishing,
    Published,
    DbCommitting,
    LibraryCommitted,
    SourceArchiving,
    SourceArchived,
    CleanupRequired,
    Conflict,
    Failed(String),
    Cancelled,
}

impl fmt::Display for TransactionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Planned => write!(f, "planned"),
            Self::Staging => write!(f, "staging"),
            Self::Verifying => write!(f, "verifying"),
            Self::Verified => write!(f, "verified"),
            Self::Publishing => write!(f, "publishing"),
            Self::Published => write!(f, "published"),
            Self::DbCommitting => write!(f, "db_committing"),
            Self::LibraryCommitted => write!(f, "library_committed"),
            Self::SourceArchiving => write!(f, "source_archiving"),
            Self::SourceArchived => write!(f, "source_archived"),
            Self::CleanupRequired => write!(f, "cleanup_required"),
            Self::Conflict => write!(f, "conflict"),
            Self::Failed(msg) => write!(f, "failed: {msg}"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

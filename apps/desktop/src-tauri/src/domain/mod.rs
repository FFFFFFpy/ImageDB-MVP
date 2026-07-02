use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
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
    pub pgvector_available: bool,
    pub can_create_tables: bool,
    pub diagnostics: Vec<String>,
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
    Ready,
    Staging,
    Verifying,
    Verified,
    Publishing,
    Published,
    DbCommitting,
    Committed,
    SourceArchiving,
    SourceArchived,
    Failed(String),
}

impl fmt::Display for TransactionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready => write!(f, "READY"),
            Self::Staging => write!(f, "STAGING"),
            Self::Verifying => write!(f, "VERIFYING"),
            Self::Verified => write!(f, "VERIFIED"),
            Self::Publishing => write!(f, "PUBLISHING"),
            Self::Published => write!(f, "PUBLISHED"),
            Self::DbCommitting => write!(f, "DB_COMMITTING"),
            Self::Committed => write!(f, "COMMITTED"),
            Self::SourceArchiving => write!(f, "SOURCE_ARCHIVING"),
            Self::SourceArchived => write!(f, "SOURCE_ARCHIVED"),
            Self::Failed(msg) => write!(f, "FAILED: {msg}"),
        }
    }
}

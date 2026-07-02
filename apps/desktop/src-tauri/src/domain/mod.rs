use serde::{Deserialize, Serialize};
use std::fmt;

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

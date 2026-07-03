//! Centralized state-machine definitions for import runs, file transactions,
//! file operations, and import plans. Every state change in the commit and
//! recovery pipelines must go through these transition functions; services
//! never write unchecked state strings.
//!
//! Some enum variants and transition functions are defined for completeness
//! and future pipeline stages; they are allowed to be unused until wired in.
#![allow(dead_code)]
use crate::error::AppError;
use std::fmt;

/// Error returned for invalid state transitions.
#[derive(Debug, Clone)]
pub struct StateError {
    pub current: String,
    pub action: String,
    pub message: String,
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid transition: cannot {} from state {}: {}",
            self.action, self.current, self.message
        )
    }
}

impl From<StateError> for AppError {
    fn from(e: StateError) -> Self {
        AppError::Internal(e.to_string())
    }
}

/// Transaction state for file operations within an album commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Failed,
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
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl TransactionState {
    /// Parse a transaction state string. `failed` may carry a `": message"`
    /// suffix in some legacy rows; the base token is what we match on.
    pub fn parse(s: &str) -> Result<Self, StateError> {
        let base = s.split(':').next().unwrap_or("").trim();
        match base {
            "planned" => Ok(Self::Planned),
            "staging" => Ok(Self::Staging),
            "verifying" => Ok(Self::Verifying),
            "verified" => Ok(Self::Verified),
            "publishing" => Ok(Self::Publishing),
            "published" => Ok(Self::Published),
            "db_committing" => Ok(Self::DbCommitting),
            "library_committed" => Ok(Self::LibraryCommitted),
            "source_archiving" => Ok(Self::SourceArchiving),
            "source_archived" => Ok(Self::SourceArchived),
            "cleanup_required" => Ok(Self::CleanupRequired),
            "conflict" => Ok(Self::Conflict),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(StateError {
                current: s.to_string(),
                action: "parse".to_string(),
                message: format!("unknown transaction state '{other}'"),
            }),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::SourceArchived | Self::Failed | Self::Cancelled)
    }
}

/// Plan state for import plans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanState {
    Draft,
    Frozen,
    Consumed,
    Invalidated,
}

impl fmt::Display for PlanState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Draft => write!(f, "draft"),
            Self::Frozen => write!(f, "frozen"),
            Self::Consumed => write!(f, "consumed"),
            Self::Invalidated => write!(f, "invalidated"),
        }
    }
}

impl PlanState {
    pub fn parse(s: &str) -> Result<Self, StateError> {
        match s.trim() {
            "draft" => Ok(Self::Draft),
            "frozen" => Ok(Self::Frozen),
            "consumed" => Ok(Self::Consumed),
            "invalidated" => Ok(Self::Invalidated),
            other => Err(StateError {
                current: s.to_string(),
                action: "parse".to_string(),
                message: format!("unknown plan state '{other}'"),
            }),
        }
    }
}

/// File operation state for individual file copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOpState {
    Planned,
    Copying,
    Copied,
    Verifying,
    Verified,
    Published,
    Failed,
    Cancelled,
}

impl fmt::Display for FileOpState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Planned => write!(f, "planned"),
            Self::Copying => write!(f, "copying"),
            Self::Copied => write!(f, "copied"),
            Self::Verifying => write!(f, "verifying"),
            Self::Verified => write!(f, "verified"),
            Self::Published => write!(f, "published"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl FileOpState {
    pub fn parse(s: &str) -> Result<Self, StateError> {
        match s.trim() {
            "planned" => Ok(Self::Planned),
            "copying" => Ok(Self::Copying),
            "copied" => Ok(Self::Copied),
            "verifying" => Ok(Self::Verifying),
            "verified" => Ok(Self::Verified),
            "published" => Ok(Self::Published),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(StateError {
                current: s.to_string(),
                action: "parse".to_string(),
                message: format!("unknown file operation state '{other}'"),
            }),
        }
    }
}

/// Validated transitions for a single file operation.
/// Legal path: planned → copying → copied → verifying → verified → published.
pub fn next_file_op_state(current: &FileOpState, action: &str) -> Result<FileOpState, StateError> {
    match (current, action) {
        (FileOpState::Planned, "copy") => Ok(FileOpState::Copying),
        (FileOpState::Copying, "copied") => Ok(FileOpState::Copied),
        (FileOpState::Copied, "verify") => Ok(FileOpState::Verifying),
        (FileOpState::Verifying, "verified") => Ok(FileOpState::Verified),
        (FileOpState::Verified, "publish") => Ok(FileOpState::Published),
        (FileOpState::Planned, "verified") => Ok(FileOpState::Verified),
        (FileOpState::Verified, "retry") => Ok(FileOpState::Verified),
        (FileOpState::Published, "retry") => Ok(FileOpState::Published),
        (_, "fail") => Ok(FileOpState::Failed),
        (FileOpState::Planned | FileOpState::Copying, "cancel") => Ok(FileOpState::Cancelled),
        _ => Err(StateError {
            current: current.to_string(),
            action: action.to_string(),
            message: format!(
                "no file operation transition defined from '{}' via '{}'",
                current, action
            ),
        }),
    }
}

/// Validated state transitions for import runs.
pub fn next_import_run_state(current: &str, action: &str) -> Result<&'static str, StateError> {
    match (current, action) {
        // Created → Scanning (start scan)
        ("created", "start_scan") => Ok("scanning"),
        // Scanning → Fingerprinting (albums discovered)
        ("scanning", "fingerprint") => Ok("fingerprinting"),
        // Fingerprinting → DetectingDuplicates (fingerprints complete)
        ("fingerprinting", "detect") => Ok("detecting_duplicates"),
        // DetectingDuplicates → Analyzing (intra-album done)
        ("detecting_duplicates", "analyze") => Ok("analyzing"),
        // Analyzing → ReviewRequired (has candidates needing review)
        ("analyzing", "require_review") => Ok("review_required"),
        // Analyzing → ReadyToCommit (no candidates needing review)
        ("analyzing", "ready") => Ok("ready_to_commit"),
        // ReviewRequired → ReadyToCommit (all decisions made)
        ("review_required", "finalize") => Ok("ready_to_commit"),
        // ReadyToCommit → Committing (start commit)
        ("ready_to_commit", "commit") => Ok("committing"),
        // Committing → RecoveryRequired (interrupted)
        ("committing", "recover") => Ok("recovery_required"),
        // Committing → Completed (all done)
        ("committing", "complete") => Ok("completed"),
        // RecoveryRequired → Committing (retry)
        ("recovery_required", "retry") => Ok("committing"),
        // RecoveryRequired → Completed (resolved)
        ("recovery_required", "resolve") => Ok("completed"),
        // Any → Cancelled
        (_, "cancel") => Ok("cancelled"),
        // Any → Failed
        (_, "fail") => Ok("failed"),
        _ => Err(StateError {
            current: current.to_string(),
            action: action.to_string(),
            message: format!("no transition defined from '{current}' via '{action}'"),
        }),
    }
}

/// Validated state transitions for file transactions.
pub fn next_transaction_state(current: &str, action: &str) -> Result<&'static str, StateError> {
    match (current, action) {
        ("planned", "stage") => Ok("staging"),
        ("staging", "verify") => Ok("verifying"),
        ("verifying", "verified") => Ok("verified"),
        ("verified", "publish") => Ok("publishing"),
        ("publishing", "published") => Ok("published"),
        ("published", "db_commit") => Ok("db_committing"),
        ("db_committing", "library_committed") => Ok("library_committed"),
        ("library_committed", "archive") => Ok("source_archiving"),
        ("source_archiving", "archived") => Ok("source_archived"),
        // Error transitions
        (_, "fail") => Ok("failed"),
        (_, "cancel") => Ok("cancelled"),
        (_, "conflict") => Ok("conflict"),
        // Recovery
        ("staging", "retry") => Ok("staging"),
        ("verified", "retry") => Ok("verified"),
        ("published", "retry") => Ok("published"),
        ("db_committing", "retry") => Ok("db_committing"),
        ("library_committed", "retry") => Ok("library_committed"),
        ("source_archiving", "retry") => Ok("source_archiving"),
        ("cleanup_required", "clean") => Ok("cleanup_required"),
        ("cleanup_required", "cleaned") => Ok("source_archived"),
        _ => Err(StateError {
            current: current.to_string(),
            action: action.to_string(),
            message: format!("no transaction transition defined from '{current}' via '{action}'"),
        }),
    }
}

/// Typed variant of [`next_transaction_state`]: returns the enum directly so
/// callers cannot accidentally write an unchecked state string. This is the
/// function the commit pipeline and recovery service must use.
pub fn transition_transaction(
    current: TransactionState,
    action: &str,
) -> Result<TransactionState, StateError> {
    let next_str = next_transaction_state(&current.to_string(), action)?;
    TransactionState::parse(next_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_run_valid_transitions() {
        assert_eq!(
            next_import_run_state("created", "start_scan").unwrap(),
            "scanning"
        );
        assert_eq!(
            next_import_run_state("scanning", "fingerprint").unwrap(),
            "fingerprinting"
        );
        assert_eq!(
            next_import_run_state("fingerprinting", "detect").unwrap(),
            "detecting_duplicates"
        );
        assert_eq!(
            next_import_run_state("detecting_duplicates", "analyze").unwrap(),
            "analyzing"
        );
        assert_eq!(
            next_import_run_state("analyzing", "ready").unwrap(),
            "ready_to_commit"
        );
        assert_eq!(
            next_import_run_state("analyzing", "require_review").unwrap(),
            "review_required"
        );
        assert_eq!(
            next_import_run_state("review_required", "finalize").unwrap(),
            "ready_to_commit"
        );
        assert_eq!(
            next_import_run_state("ready_to_commit", "commit").unwrap(),
            "committing"
        );
        assert_eq!(
            next_import_run_state("committing", "complete").unwrap(),
            "completed"
        );
        assert_eq!(
            next_import_run_state("committing", "recover").unwrap(),
            "recovery_required"
        );
        assert_eq!(
            next_import_run_state("recovery_required", "retry").unwrap(),
            "committing"
        );
        assert_eq!(
            next_import_run_state("recovery_required", "resolve").unwrap(),
            "completed"
        );
    }

    #[test]
    fn import_run_cancel_from_any() {
        for state in &[
            "created",
            "scanning",
            "fingerprinting",
            "detecting_duplicates",
            "analyzing",
            "review_required",
            "ready_to_commit",
            "committing",
            "recovery_required",
        ] {
            assert_eq!(next_import_run_state(state, "cancel").unwrap(), "cancelled");
        }
    }

    #[test]
    fn import_run_fail_from_any() {
        for state in &[
            "created",
            "scanning",
            "fingerprinting",
            "detecting_duplicates",
            "analyzing",
            "review_required",
            "ready_to_commit",
            "committing",
            "recovery_required",
        ] {
            assert_eq!(next_import_run_state(state, "fail").unwrap(), "failed");
        }
    }

    #[test]
    fn import_run_invalid_transition() {
        let err = next_import_run_state("completed", "commit").unwrap_err();
        assert!(err.to_string().contains("completed"));
    }

    #[test]
    fn transaction_valid_transitions() {
        assert_eq!(
            next_transaction_state("planned", "stage").unwrap(),
            "staging"
        );
        assert_eq!(
            next_transaction_state("staging", "verify").unwrap(),
            "verifying"
        );
        assert_eq!(
            next_transaction_state("verifying", "verified").unwrap(),
            "verified"
        );
        assert_eq!(
            next_transaction_state("verified", "publish").unwrap(),
            "publishing"
        );
        assert_eq!(
            next_transaction_state("publishing", "published").unwrap(),
            "published"
        );
        assert_eq!(
            next_transaction_state("published", "db_commit").unwrap(),
            "db_committing"
        );
        assert_eq!(
            next_transaction_state("db_committing", "library_committed").unwrap(),
            "library_committed"
        );
        assert_eq!(
            next_transaction_state("library_committed", "archive").unwrap(),
            "source_archiving"
        );
        assert_eq!(
            next_transaction_state("source_archiving", "archived").unwrap(),
            "source_archived"
        );
    }

    #[test]
    fn transaction_fail_from_any() {
        for state in &[
            "planned",
            "staging",
            "verifying",
            "verified",
            "publishing",
            "published",
            "db_committing",
            "library_committed",
            "source_archiving",
        ] {
            assert_eq!(next_transaction_state(state, "fail").unwrap(), "failed");
        }
    }

    #[test]
    fn transaction_invalid_transition() {
        let err = next_transaction_state("planned", "publish").unwrap_err();
        assert!(err.to_string().contains("planned"));
    }

    #[test]
    fn state_error_implements_app_error() {
        let err = StateError {
            current: "test".to_string(),
            action: "invalid".to_string(),
            message: "test error".to_string(),
        };
        let app_err: AppError = err.into();
        assert!(app_err.to_string().contains("test error"));
    }
}

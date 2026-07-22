#![allow(dead_code)]

use crate::domain::import_state::{
    Decision, DecisionSource, DecodeState, DuplicateScope, ImportAlbumState, ImportImageState,
    ImportPlan, ImportPlanAlbum, ImportPlanImage, ImportRunState, MatchType, SourceFileMode,
    SCAN_POLICY_VERSION,
};
use crate::domain::state_machine::{FileOpState, PlanState, TransactionState};
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tokio_postgres::Client;
use uuid::Uuid;

fn postgres_error_detail(error: &tokio_postgres::Error) -> String {
    error
        .as_db_error()
        .map(|db_error| format!("{}: {}", db_error.code().code(), db_error.message()))
        .unwrap_or_else(|| error.to_string())
}

/// Strip the album-name prefix (forward or backward slash) from a source
/// relative path and normalize the remainder to a forward-slash target
/// relative path. Mirrors `review_service::target_relative_path_for_album`
/// so the repository can build target paths while freezing without depending
/// on the service layer.
fn target_relative_path_for_album_name(
    album_name: &str,
    source_relative_path: &str,
) -> Result<String, AppError> {
    let slash_prefix = format!("{album_name}/");
    let backslash_prefix = format!("{album_name}\\");
    let rel = source_relative_path
        .strip_prefix(&slash_prefix)
        .or_else(|| source_relative_path.strip_prefix(&backslash_prefix))
        .unwrap_or(source_relative_path);
    crate::services::commit_service::normalize_relative_path(rel)
}

pub struct ImportRepository;

fn scan_run_state_from_facts(
    unfinished_albums: i64,
    failed_albums: i64,
    pending_reviews: i64,
) -> ImportRunState {
    if unfinished_albums > 0 {
        ImportRunState::Cancelled
    } else if failed_albums > 0 {
        ImportRunState::Failed
    } else if pending_reviews > 0 {
        ImportRunState::ReviewRequired
    } else {
        ImportRunState::ReadyToCommit
    }
}

pub struct ImportRunRecord {
    pub id: Uuid,
    pub source_root: String,
    pub library_root_id: Uuid,
    pub state: String,
    pub policy_version: String,
    pub statistics: serde_json::Value,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub struct ImportAlbumRecord {
    pub id: Uuid,
    pub source_path: String,
    pub source_name: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportAlbumStatus {
    pub id: String,
    pub import_run_id: String,
    pub source_name: String,
    pub source_path: String,
    pub state: String,
    pub image_count: i32,
    pub fingerprinted_count: i32,
    pub duplicate_candidate_count: i32,
    pub review_candidate_count: i32,
    pub last_error_message: Option<String>,
    pub analysis_started_at: Option<String>,
    pub analysis_completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRunDashboard {
    pub import_run_id: String,
    pub source_root: String,
    pub state: String,
    pub total_albums: i32,
    pub pending_albums: i32,
    pub analyzing_albums: i32,
    pub analyzed_albums: i32,
    pub review_required_albums: i32,
    pub failed_albums: i32,
    pub total_images: i32,
    pub pending_reviews: i32,
    pub duplicate_candidates: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseInfoDatabaseSection {
    pub mode: Option<String>,
    pub status: String,
    pub pgvector_available: bool,
    pub migration_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseInfoLibrarySection {
    pub library_root_count: i64,
    pub library_album_count: i64,
    pub library_image_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseInfoImportsSection {
    pub import_run_count: i64,
    pub import_album_count: i64,
    pub import_image_count: i64,
    pub pending_review_count: i64,
    pub failed_album_count: i64,
    pub recovery_required_run_count: i64,
    pub failed_run_count: i64,
    pub frozen_plan_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardNextAction {
    Recover,
    InspectTransactionFailure,
    Review,
    GeneratePlan,
    ResumeAnalysis,
    InspectFailed,
    ResumeCommit,
    NewImport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardActionableRun {
    #[serde(flatten)]
    pub run: ImportRunDashboard,
    pub next_action: DashboardNextAction,
    pub has_frozen_plan: bool,
    pub has_recoverable_transaction: bool,
    pub has_terminal_unresolved_transaction: bool,
    pub has_missing_plan_album_transaction: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseInfoDashboard {
    pub database: DatabaseInfoDatabaseSection,
    pub library: DatabaseInfoLibrarySection,
    pub imports: DatabaseInfoImportsSection,
    pub latest_run: Option<ImportRunDashboard>,
    pub latest_actionable_run: Option<DashboardActionableRun>,
    pub next_action: DashboardNextAction,
}

pub struct ImportImageRecord {
    pub id: Uuid,
    pub source_path: String,
    pub relative_path: String,
    pub file_size: i64,
    pub modified_at: Option<chrono::DateTime<chrono::Utc>>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub format: Option<String>,
    pub decode_state: String,
    pub blake3: Option<Vec<u8>>,
    pub pixel_hash: Option<Vec<u8>>,
    pub fingerprint_version: Option<String>,
    pub state: String,
}

#[derive(Debug, Clone)]
pub struct LibraryImageRow {
    pub id: Uuid,
    pub file_size: i64,
    pub blake3: Vec<u8>,
    pub pixel_hash: Option<Vec<u8>>,
    pub block_hash_16: Option<Vec<u8>>,
    pub double_gradient_hash_32: Option<Vec<u8>>,
    pub perceptual_eligible: bool,
    pub fingerprint_version: String,
}

#[derive(Debug, Clone)]
pub struct RunExactFingerprintRow {
    pub id: Uuid,
    pub album_id: Uuid,
    pub file_size: i64,
    pub blake3: Vec<u8>,
    pub pixel_hash: Vec<u8>,
}

pub struct NewImportImage {
    pub album_id: Uuid,
    pub source_path: String,
    pub relative_path: String,
    pub file_size: i64,
    pub modified_at: Option<chrono::DateTime<chrono::Utc>>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub format: Option<String>,
    pub decode_state: DecodeState,
    pub blake3: Option<Vec<u8>>,
    pub pixel_hash: Option<Vec<u8>>,
    pub block_hash_16: Option<Vec<u8>>,
    pub double_gradient_hash_32: Option<Vec<u8>>,
    pub perceptual_eligible: bool,
    pub fingerprint_version: Option<String>,
    pub state: ImportImageState,
}

pub struct NewFileOperation {
    pub source_path: String,
    pub staging_path: String,
    pub target_path: String,
    pub expected_size: i64,
    pub expected_blake3: Vec<u8>,
    pub source_cleanup_quarantine_path: Option<String>,
}

pub struct NewDuplicateCandidate {
    pub import_run_id: Uuid,
    pub source_image_id: Uuid,
    pub candidate_source_image_id: Option<Uuid>,
    pub candidate_library_image_id: Option<Uuid>,
    pub scope: DuplicateScope,
    pub match_type: MatchType,
    pub blake3_equal: bool,
    pub pixel_hash_equal: bool,
    pub block_distance: Option<i32>,
    pub double_gradient_distance: Option<i32>,
    pub block_distance_ratio: Option<f64>,
    pub double_gradient_distance_ratio: Option<f64>,
    pub transform_type: Option<String>,
    pub confidence: Option<f64>,
    pub decision: Option<Decision>,
    pub decision_source: Option<DecisionSource>,
}

pub struct ReviewCandidateRow {
    pub candidate_id: Uuid,
    pub source_image_id: Uuid,
    pub candidate_source_image_id: Option<Uuid>,
    pub candidate_library_image_id: Option<Uuid>,
    pub scope: String,
    pub match_type: String,
    pub transform_type: Option<String>,
    pub confidence: Option<f64>,
    pub album_name: String,
    pub has_decision: bool,
}

pub struct ReviewCandidateDetailRow {
    pub candidate_id: Uuid,
    pub source_image_id: Uuid,
    pub source_image_path: String,
    pub source_image_file_size: i64,
    pub source_image_width: Option<i32>,
    pub source_image_height: Option<i32>,
    pub candidate_source_image_id: Option<Uuid>,
    pub candidate_source_image_path: Option<String>,
    pub candidate_source_image_file_size: Option<i64>,
    pub candidate_source_image_width: Option<i32>,
    pub candidate_source_image_height: Option<i32>,
    pub candidate_library_image_id: Option<Uuid>,
    pub candidate_library_image_path: Option<String>,
    pub candidate_library_image_file_size: Option<i64>,
    pub candidate_library_image_width: Option<i32>,
    pub candidate_library_image_height: Option<i32>,
    pub scope: String,
    pub match_type: String,
    pub blake3_equal: bool,
    pub pixel_hash_equal: bool,
    pub block_distance: Option<i32>,
    pub double_gradient_distance: Option<i32>,
    pub block_distance_ratio: Option<f64>,
    pub double_gradient_distance_ratio: Option<f64>,
    pub transform_type: Option<String>,
    pub confidence: Option<f64>,
    pub album_name: String,
    pub album_id: Uuid,
    pub existing_decision: Option<String>,
}

pub struct ReviewProgressRow {
    pub total: u32,
    pub decided: u32,
}

#[derive(Debug, Clone)]
pub struct ReviewGroupSummaryRow {
    pub group_id: Uuid,
    pub state: String,
    pub requires_manual_review: bool,
    pub member_count: i64,
    pub import_member_count: i64,
    pub library_member_count: i64,
    pub kept_count: i64,
}

#[derive(Debug, Clone)]
pub struct ReviewGroupMemberRow {
    pub image_id: Uuid,
    pub image_source: String,
    pub final_action: String,
    pub decision_source: String,
    pub source_path: String,
    pub relative_path: String,
    pub album_name: String,
    pub file_size: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub format: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReviewGroupEvidenceRow {
    pub candidate_id: Uuid,
    pub source_image_id: Uuid,
    pub candidate_image_id: Uuid,
    pub candidate_image_source: String,
    pub scope: String,
    pub match_type: String,
    pub blake3_equal: bool,
    pub pixel_hash_equal: bool,
    pub block_distance: Option<i32>,
    pub double_gradient_distance: Option<i32>,
    pub block_distance_ratio: Option<f64>,
    pub double_gradient_distance_ratio: Option<f64>,
    pub transform_type: Option<String>,
    pub confidence: Option<f64>,
    pub automatic: bool,
}

pub struct ImportPlanCandidateRow {
    pub candidate_id: Uuid,
    pub source_image_id: Uuid,
    pub candidate_source_image_id: Option<Uuid>,
    pub candidate_library_image_id: Option<Uuid>,
    pub scope: String,
    pub candidate_decision: Option<String>,
    pub review_decision: Option<String>,
    pub source_album_id: Uuid,
    pub blake3_equal: bool,
    pub pixel_hash_equal: bool,
    pub confidence: Option<f64>,
}

pub struct ImportPlanImageRow {
    pub id: Uuid,
    pub source_path: String,
    pub relative_path: String,
    pub file_size: i64,
    pub album_id: Uuid,
    pub album_name: String,
}

pub struct AlbumRow {
    pub id: Uuid,
    pub source_name: String,
}

pub struct ImportAlbumFullRow {
    pub id: Uuid,
    pub source_path: String,
    pub source_name: String,
    pub state: String,
}

#[derive(Clone)]
pub struct ImportImageFullRow {
    pub id: Uuid,
    pub source_path: String,
    pub relative_path: String,
    pub file_size: i64,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub format: Option<String>,
    pub blake3: Option<Vec<u8>>,
    pub pixel_hash: Option<Vec<u8>>,
    pub block_hash_16: Option<Vec<u8>>,
    pub double_gradient_hash_32: Option<Vec<u8>>,
    pub fingerprint_version: Option<String>,
    pub import_album_id: Uuid,
}

pub struct LibraryAlbumRow {
    pub id: Uuid,
    pub image_count: i32,
    pub state: String,
}

pub struct FileTransactionRow {
    pub id: Uuid,
    pub state: String,
}

/// Minimal transaction projection used by parent-run reconciliation.
///
/// Carries just enough to decide whether the parent `import_runs.state`
/// must be `recovery_required` (conflict / active / failed / cancelled)
/// or `completed` (every frozen-plan album reached `source_archived`).
pub struct FileTransactionStateRow {
    pub id: Uuid,
    pub import_run_id: Uuid,
    pub import_album_id: Uuid,
    pub state: String,
}

/// A frozen plan album with its persisted target path.
#[derive(Debug, Clone)]
pub struct PlanAlbumRow {
    pub plan_album_id: Uuid,
    pub import_album_id: Uuid,
    pub target_relative_path: String,
    pub expected_image_count: i32,
    pub album_plan_hash: Option<Vec<u8>>,
}

/// A single persisted plan image entry.
#[derive(Debug, Clone)]
pub struct PlanImageRow {
    pub id: Uuid,
    pub plan_album_id: Uuid,
    pub import_image_id: Uuid,
    pub source_path: String,
    pub source_relative_path: String,
    pub target_relative_path: String,
    pub expected_file_size: i64,
    pub expected_blake3: Vec<u8>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub format: Option<String>,
}

/// Full frozen plan: header + albums (each with its images).
#[derive(Debug, Clone)]
pub struct FrozenPlanRow {
    pub plan_id: Uuid,
    pub import_run_id: Uuid,
    pub library_root_id: Uuid,
    pub plan_state: String,
    pub plan_hash: Option<Vec<u8>>,
    pub policy_version: String,
    pub source_file_mode: SourceFileMode,
    pub albums: Vec<(PlanAlbumRow, Vec<PlanImageRow>)>,
}

// ── File transaction recovery row types ───────────────────────────────

/// Full persisted record of a file transaction.
#[derive(Debug, Clone)]
pub struct FileTransactionFullRow {
    pub id: Uuid,
    pub import_run_id: Uuid,
    pub import_album_id: Uuid,
    pub state: String,
    pub staging_path: Option<String>,
    pub target_path: Option<String>,
    pub manifest_path: Option<String>,
    pub plan_hash: Option<Vec<u8>>,
    pub manifest_hash: Option<Vec<u8>>,
    pub source_file_mode: SourceFileMode,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SourceFileCleanupOperationRow {
    pub id: Uuid,
    pub transaction_id: Uuid,
    pub source_path: String,
    pub quarantine_path: Option<String>,
    pub expected_size: i64,
    pub expected_blake3: Vec<u8>,
    pub state: String,
    pub last_error: Option<String>,
}

/// A persisted file operation with all prewritten evidence.
#[derive(Debug, Clone)]
pub struct FileOperationRow {
    pub id: Uuid,
    pub transaction_id: Uuid,
    pub source_path: String,
    pub staging_path: String,
    pub target_path: String,
    pub expected_size: i64,
    pub expected_blake3: Vec<u8>,
    pub actual_blake3: Option<Vec<u8>>,
    pub state: String,
    pub last_error: Option<String>,
}

/// A persisted source album snapshot header.
#[derive(Debug, Clone)]
pub struct SourceAlbumSnapshotRecord {
    pub snapshot_id: Uuid,
    pub import_run_id: Uuid,
    pub import_album_id: Uuid,
    pub source_album_path: String,
    pub snapshot_hash: Vec<u8>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// A persisted snapshot file entry.
#[derive(Debug, Clone)]
pub struct SnapshotFileRecord {
    pub id: Uuid,
    pub snapshot_id: Uuid,
    pub relative_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub blake3: Vec<u8>,
}

/// Input struct for inserting a snapshot file.
#[derive(Debug)]
pub struct NewSnapshotFile {
    pub relative_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub blake3: Vec<u8>,
}

/// Full library album record (for idempotency verification).
#[derive(Debug, Clone)]
pub struct LibraryAlbumFullRow {
    pub id: Uuid,
    pub library_root_id: Uuid,
    pub display_name: String,
    pub relative_path: String,
    pub manifest_version: String,
    pub manifest_hash: Vec<u8>,
    pub image_count: i32,
    pub state: String,
    pub plan_hash: Option<Vec<u8>>,
    pub transaction_id: Option<Uuid>,
}

/// Full library image record (for idempotency verification).
#[derive(Debug, Clone)]
pub struct LibraryImageFullRow {
    pub id: Uuid,
    pub relative_path: String,
    pub file_size: i64,
    pub blake3: Vec<u8>,
    pub state: String,
}

#[derive(Debug, Clone)]
pub struct LibraryAlbumDetailRow {
    pub id: Uuid,
    pub library_root_id: Uuid,
    pub library_root_path: String,
    pub display_name: String,
    pub relative_path: String,
    pub image_count: i32,
    pub total_size: i64,
    pub state: String,
    pub committed_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct LibraryAlbumKeyset {
    pub committed_at: chrono::DateTime<chrono::Utc>,
    pub display_name: String,
    pub id: Uuid,
}

#[derive(Debug, Clone)]
pub struct LibraryImageDetailRow {
    pub id: Uuid,
    pub relative_path: String,
    pub file_size: i64,
    pub width: i32,
    pub height: i32,
    pub format: String,
    pub state: String,
}

#[derive(Debug, Clone)]
pub struct LibraryImageKeyset {
    pub relative_path: String,
    pub id: Uuid,
}

#[derive(Debug, Clone)]
pub struct LibraryRootLeaseRow {
    pub library_root_id: Uuid,
    pub owner_instance_id: String,
    pub lease_token: Uuid,
    pub heartbeat_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

impl ImportRepository {
    pub async fn upsert_default_library_root(client: &Client) -> Result<Uuid, AppError> {
        if let Some(row) = client
            .query_opt(
                "SELECT id FROM library_roots
                 WHERE display_name = '_default_'
                 ORDER BY created_at
                 LIMIT 1",
                &[],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query default library root: {e}")))?
        {
            return Ok(row.get("id"));
        }

        let id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO library_roots (id, path, display_name, is_active)
                 VALUES ($1, '', '_default_', TRUE)",
                &[&id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to insert default library root: {e}"))
            })?;

        Ok(id)
    }

    pub async fn create_import_run(
        client: &Client,
        source_root: &str,
        library_root_id: Uuid,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        let state = ImportRunState::Scanning.to_string();
        client
            .execute(
                "INSERT INTO import_runs (id, source_root, library_root_id, state, policy_version, statistics)
                 VALUES ($1, $2, $3, $4, $5, '{}'::jsonb)",
                &[&id, &source_root, &library_root_id, &state, &SCAN_POLICY_VERSION.to_string()],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to create import run: {e}")))?;
        Ok(id)
    }

    pub async fn acquire_library_root_lease(
        client: &Client,
        library_root_id: Uuid,
        owner_instance_id: &str,
        lease_token: Uuid,
        ttl_secs: i64,
    ) -> Result<LibraryRootLeaseRow, AppError> {
        if ttl_secs <= 0 {
            return Err(AppError::Internal(
                "library root lease ttl must be positive".to_string(),
            ));
        }
        let ttl = ttl_secs as f64;
        let row = client
            .query_opt(
                "INSERT INTO library_root_leases
                    (library_root_id, owner_instance_id, lease_token, heartbeat_at, expires_at)
                 VALUES ($1, $2, $3, now(), now() + ($4 * interval '1 second'))
                 ON CONFLICT (library_root_id) DO UPDATE SET
                    owner_instance_id = EXCLUDED.owner_instance_id,
                    lease_token = EXCLUDED.lease_token,
                    heartbeat_at = now(),
                    expires_at = EXCLUDED.expires_at,
                    updated_at = now()
                 WHERE library_root_leases.expires_at <= now()
                    OR library_root_leases.owner_instance_id = EXCLUDED.owner_instance_id
                    OR library_root_leases.lease_token = EXCLUDED.lease_token
                 RETURNING library_root_id, owner_instance_id, lease_token, heartbeat_at, expires_at",
                &[&library_root_id, &owner_instance_id, &lease_token, &ttl],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to acquire library root lease: {e}")))?;

        if let Some(row) = row {
            return Ok(Self::lease_row_from_row(&row));
        }

        let holder = Self::get_library_root_lease(client, library_root_id).await?;
        let msg = match holder {
            Some(lease) => format!(
                "library root {library_root_id} is already leased by owner '{}' until {}",
                lease.owner_instance_id, lease.expires_at
            ),
            None => format!("library root {library_root_id} lease is held by another writer"),
        };
        Err(AppError::Internal(msg))
    }

    pub async fn heartbeat_library_root_lease(
        client: &Client,
        library_root_id: Uuid,
        lease_token: Uuid,
        ttl_secs: i64,
    ) -> Result<LibraryRootLeaseRow, AppError> {
        if ttl_secs <= 0 {
            return Err(AppError::Internal(
                "library root lease ttl must be positive".to_string(),
            ));
        }
        let ttl = ttl_secs as f64;
        let row = client
            .query_opt(
                "UPDATE library_root_leases
                 SET heartbeat_at = now(),
                     expires_at = now() + ($3 * interval '1 second'),
                     updated_at = now()
                 WHERE library_root_id = $1 AND lease_token = $2
                 RETURNING library_root_id, owner_instance_id, lease_token, heartbeat_at, expires_at",
                &[&library_root_id, &lease_token, &ttl],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to heartbeat library root lease: {e}")))?;

        row.map(|row| Self::lease_row_from_row(&row))
            .ok_or_else(|| {
                AppError::Internal(format!(
                "library root {library_root_id} lease heartbeat failed; token is no longer owner"
            ))
            })
    }

    pub async fn release_library_root_lease(
        client: &Client,
        library_root_id: Uuid,
        lease_token: Uuid,
    ) -> Result<(), AppError> {
        client
            .execute(
                "DELETE FROM library_root_leases WHERE library_root_id = $1 AND lease_token = $2",
                &[&library_root_id, &lease_token],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to release library root lease: {e}"))
            })?;
        Ok(())
    }

    pub async fn get_library_root_lease(
        client: &Client,
        library_root_id: Uuid,
    ) -> Result<Option<LibraryRootLeaseRow>, AppError> {
        let row = client
            .query_opt(
                "SELECT library_root_id, owner_instance_id, lease_token, heartbeat_at, expires_at
                 FROM library_root_leases WHERE library_root_id = $1",
                &[&library_root_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to get library root lease: {e}")))?;
        Ok(row.map(|row| Self::lease_row_from_row(&row)))
    }

    fn lease_row_from_row(row: &tokio_postgres::Row) -> LibraryRootLeaseRow {
        LibraryRootLeaseRow {
            library_root_id: row.get("library_root_id"),
            owner_instance_id: row.get("owner_instance_id"),
            lease_token: row.get("lease_token"),
            heartbeat_at: row.get("heartbeat_at"),
            expires_at: row.get("expires_at"),
        }
    }

    pub async fn update_import_run_state(
        client: &Client,
        id: Uuid,
        state: &ImportRunState,
    ) -> Result<(), AppError> {
        let state_str = state.to_string();
        let completed_at = match state {
            ImportRunState::Completed
            | ImportRunState::Cancelled
            | ImportRunState::Failed
            | ImportRunState::Abandoned => Some(chrono::Utc::now()),
            _ => None,
        };
        client
            .execute(
                "UPDATE import_runs SET state = $1, completed_at = COALESCE($2, completed_at) WHERE id = $3",
                &[&state_str, &completed_at, &id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to update import run state: {e}")))?;
        Ok(())
    }

    /// Resolve a cancellation race from persisted album/candidate facts. A
    /// late cancel after the last album checkpoint must not strand a complete
    /// run in `cancelled` with no work left to resume.
    pub async fn reconcile_scan_run_state_after_cancellation(
        client: &Client,
        id: Uuid,
    ) -> Result<ImportRunState, AppError> {
        let row = client
            .query_one(
                "SELECT
                    COUNT(*) FILTER (WHERE state IN ('pending', 'analyzing', 'scanning', 'fingerprinting'))::BIGINT AS unfinished,
                    COUNT(*) FILTER (WHERE state = 'failed')::BIGINT AS failed,
                    (SELECT CASE
                        WHEN EXISTS (SELECT 1 FROM review_groups WHERE import_run_id = $1)
                        THEN (SELECT COUNT(*) FROM review_groups
                              WHERE import_run_id = $1
                                AND requires_manual_review
                                AND state = 'pending')
                        ELSE (SELECT COUNT(*)
                              FROM duplicate_candidates dc
                              LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                              WHERE dc.import_run_id = $1
                                AND dc.decision IS NULL
                                AND rd.id IS NULL)
                     END)::BIGINT AS pending_reviews
                 FROM import_albums
                 WHERE import_run_id = $1",
                &[&id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to reconcile cancelled scan: {e}")))?;

        let state = scan_run_state_from_facts(
            row.get("unfinished"),
            row.get("failed"),
            row.get("pending_reviews"),
        );
        Self::update_import_run_state(client, id, &state).await?;
        Ok(state)
    }

    /// Preserve a run's checkpoint as historical evidence while making it
    /// explicit that it must never be resumed. Frozen plans and file
    /// transactions remain fail-closed and cannot be abandoned here.
    pub async fn abandon_import_run(client: &Client, id: Uuid) -> Result<(), AppError> {
        client
            .batch_execute("BEGIN")
            .await
            .map_err(|e| AppError::Internal(format!("failed to begin import run abandon: {e}")))?;
        let result = async {
            let row = client
                .query_opt(
                    "SELECT state,
                            EXISTS (SELECT 1 FROM import_plans WHERE import_run_id = $1) AS has_plan,
                            EXISTS (SELECT 1 FROM file_transactions WHERE import_run_id = $1) AS has_transaction
                     FROM import_runs WHERE id = $1 FOR UPDATE",
                    &[&id],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to lock import run for abandon: {e}")))?
                .ok_or_else(|| AppError::Internal(format!("import run {id} was not found")))?;
            let state: String = row.get("state");
            if !matches!(state.as_str(), "analyzing" | "scanning" | "fingerprinting" | "cancelled" | "failed") {
                return Err(AppError::Internal(format!(
                    "import run {id} cannot be abandoned from state '{state}'"
                )));
            }
            if row.get::<_, bool>("has_plan") || row.get::<_, bool>("has_transaction") {
                return Err(AppError::Internal(
                    "cannot abandon an import run referenced by a plan or file transaction".to_string(),
                ));
            }
            let updated = client
                .execute(
                    "UPDATE import_runs SET state = 'abandoned', completed_at = now()
                     WHERE id = $1 AND state = $2",
                    &[&id, &state],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to abandon import run: {e}")))?;
            if updated != 1 {
                return Err(AppError::Internal(format!("import run {id} changed while abandoning")));
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                client.batch_execute("COMMIT").await.map_err(|e| {
                    AppError::Internal(format!("failed to commit import run abandon: {e}"))
                })?;
                Ok(())
            }
            Err(e) => {
                let _ = client.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    /// Reopen a scan-phase run for analysis and clear terminal metadata left
    /// by a prior cancellation/failure.  The guarded UPDATE prevents callers
    /// from turning a reviewed, frozen, committing, or completed run back into
    /// a mutable scan run.
    pub async fn reopen_import_run_for_analysis(client: &Client, id: Uuid) -> Result<(), AppError> {
        let state = ImportRunState::Analyzing.to_string();
        let updated = client
            .execute(
                "UPDATE import_runs
                 SET state = $1,
                     completed_at = NULL,
                     error_code = NULL,
                     error_message = NULL
                 WHERE id = $2
                   AND state IN ('analyzing', 'scanning', 'fingerprinting', 'cancelled', 'failed')",
                &[&state, &id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to reopen import run: {e}")))?;
        if updated != 1 {
            return Err(AppError::Internal(format!(
                "import run {id} is no longer resumable"
            )));
        }
        Ok(())
    }

    /// Update an import run's state with explicit control over `completed_at`.
    ///
    /// - `Some(ts)` writes the timestamp verbatim (used when reconcile
    ///   determines the run is now `completed` and needs a stable value).
    /// - `None` clears the timestamp (used when reconcile pulls a run back
    ///   from `completed` to `recovery_required` so the row no longer claims
    ///   the run finished).
    ///
    /// This is the only updater reconciliation uses, because the COALESCE
    /// semantics of `update_import_run_state` cannot clear a previously set
    /// `completed_at`.
    pub async fn set_import_run_state(
        client: &Client,
        id: Uuid,
        state: &ImportRunState,
        completed_at: Option<chrono::DateTime<chrono::Utc>>,
        clear_completed_at: bool,
    ) -> Result<(), AppError> {
        let state_str = state.to_string();
        if clear_completed_at {
            client
                .execute(
                    "UPDATE import_runs SET state = $1, completed_at = NULL WHERE id = $2",
                    &[&state_str, &id],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to clear import run completed_at: {e}"))
                })?;
        } else {
            client
                .execute(
                    "UPDATE import_runs SET state = $1, completed_at = COALESCE($2, completed_at) WHERE id = $3",
                    &[&state_str, &completed_at, &id],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to set import run state: {e}"))
                })?;
        }
        Ok(())
    }

    pub async fn update_import_run_error(
        client: &Client,
        id: Uuid,
        error_code: &str,
        error_message: &str,
    ) -> Result<(), AppError> {
        let state = ImportRunState::Failed.to_string();
        client
            .execute(
                "UPDATE import_runs SET state = $1, error_code = $2, error_message = $3, completed_at = now() WHERE id = $4",
                &[&state, &error_code, &error_message, &id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to update import run error: {e}")))?;
        Ok(())
    }

    pub async fn update_import_run_statistics(
        client: &Client,
        id: Uuid,
        statistics: &serde_json::Value,
    ) -> Result<(), AppError> {
        client
            .execute(
                "UPDATE import_runs SET statistics = $1 WHERE id = $2",
                &[statistics, &id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to update import run statistics: {e}"))
            })?;
        Ok(())
    }

    pub async fn refresh_import_run_statistics(
        client: &Client,
        id: Uuid,
    ) -> Result<serde_json::Value, AppError> {
        let row = client
            .query_one(
                "SELECT
                    (SELECT COUNT(*) FROM import_albums WHERE import_run_id = $1)::BIGINT AS total_albums,
                    (
                        SELECT COUNT(*)
                        FROM import_images ii
                        JOIN import_albums ia ON ia.id = ii.import_album_id
                        WHERE ia.import_run_id = $1
                    )::BIGINT AS total_images,
                    (SELECT COUNT(*) FROM duplicate_candidates WHERE import_run_id = $1)::BIGINT AS duplicate_count,
                    (
                        SELECT COUNT(*)
                        FROM import_albums
                        WHERE import_run_id = $1 AND state = 'failed'
                    )::BIGINT AS failed_album_count,
                    (
                        SELECT CASE
                            WHEN EXISTS (SELECT 1 FROM review_groups WHERE import_run_id = $1)
                            THEN (SELECT COUNT(*) FROM review_groups
                                  WHERE import_run_id = $1
                                    AND requires_manual_review
                                    AND state = 'pending')
                            ELSE (SELECT COUNT(*)
                                  FROM duplicate_candidates dc
                                  LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                                  WHERE dc.import_run_id = $1
                                    AND dc.decision IS NULL
                                    AND rd.id IS NULL)
                        END
                    )::BIGINT AS pending_review_count",
                &[&id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to aggregate import run statistics: {e}"))
            })?;

        let failed_album_count: i64 = row.get("failed_album_count");
        let statistics = serde_json::json!({
            "total_albums": row.get::<_, i64>("total_albums"),
            "total_images": row.get::<_, i64>("total_images"),
            "duplicate_count": row.get::<_, i64>("duplicate_count"),
            "error_count": failed_album_count,
            "failed_album_count": failed_album_count,
            "pending_review_count": row.get::<_, i64>("pending_review_count"),
        });
        Self::update_import_run_statistics(client, id, &statistics).await?;
        Ok(statistics)
    }

    pub async fn insert_import_album(
        client: &Client,
        import_run_id: Uuid,
        source_path: &str,
        source_name: &str,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        let state = ImportAlbumState::Pending.to_string();
        let row = client
            .query_one(
                "INSERT INTO import_albums (id, import_run_id, source_path, source_name, state)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (import_run_id, source_path) DO UPDATE
                 SET source_name = EXCLUDED.source_name,
                     updated_at = now()
                 RETURNING id",
                &[&id, &import_run_id, &source_path, &source_name, &state],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert import album: {e}")))?;
        Ok(row.get("id"))
    }

    pub async fn update_import_album_state(
        client: &Client,
        id: Uuid,
        state: &ImportAlbumState,
    ) -> Result<(), AppError> {
        let state_str = state.to_string();
        let started_at = if matches!(state, ImportAlbumState::Analyzing) {
            Some(chrono::Utc::now())
        } else {
            None
        };
        let completed_at = if matches!(
            state,
            ImportAlbumState::Analyzed
                | ImportAlbumState::ReviewRequired
                | ImportAlbumState::Failed
        ) {
            Some(chrono::Utc::now())
        } else {
            None
        };
        client
            .execute(
                "UPDATE import_albums
                 SET state = $1,
                     analysis_started_at = COALESCE(analysis_started_at, $2),
                     analysis_completed_at = COALESCE($3, analysis_completed_at),
                     updated_at = now()
                 WHERE id = $4",
                &[&state_str, &started_at, &completed_at, &id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to update album state: {e}")))?;
        Ok(())
    }

    pub async fn mark_import_album_analyzing(client: &Client, id: Uuid) -> Result<(), AppError> {
        let state = ImportAlbumState::Analyzing.to_string();
        client
            .execute(
                "UPDATE import_albums
                 SET state = $1,
                     analysis_started_at = COALESCE(analysis_started_at, now()),
                     analysis_completed_at = NULL,
                     last_error_code = NULL,
                     last_error_message = NULL,
                     analysis_attempts = analysis_attempts + 1,
                     updated_at = now()
                 WHERE id = $2",
                &[&state, &id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to mark album analyzing: {e}")))?;
        Ok(())
    }

    pub async fn mark_import_album_failed(
        client: &Client,
        id: Uuid,
        error_code: &str,
        error_message: &str,
    ) -> Result<(), AppError> {
        let state = ImportAlbumState::Failed.to_string();
        client
            .execute(
                "UPDATE import_albums
                 SET state = $1,
                     analysis_completed_at = now(),
                     last_error_code = $2,
                     last_error_message = $3,
                     updated_at = now()
                 WHERE id = $4",
                &[&state, &error_code, &error_message, &id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to mark album failed: {e}")))?;
        Ok(())
    }

    pub async fn refresh_album_workflow_summary(
        client: &Client,
        album_id: Uuid,
    ) -> Result<ImportAlbumStatus, AppError> {
        let row = client
            .query_one(
                "WITH counts AS (
                    SELECT
                        COUNT(ii.id)::INTEGER AS image_count,
                        COUNT(ii.id) FILTER (WHERE ii.state = 'fingerprinted')::INTEGER AS fingerprinted_count
                    FROM import_images ii
                    WHERE ii.import_album_id = $1
                 ),
                 candidate_counts AS (
                    SELECT
                        COUNT(dc.id)::INTEGER AS duplicate_candidate_count,
                        COUNT(dc.id) FILTER (
                            WHERE dc.decision IS NULL AND rd.id IS NULL
                        )::INTEGER AS review_candidate_count
                    FROM duplicate_candidates dc
                    JOIN import_images si ON dc.source_image_id = si.id
                    LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                    WHERE si.import_album_id = $1
                 )
                 UPDATE import_albums ia
                 SET image_count = counts.image_count,
                     fingerprinted_count = counts.fingerprinted_count,
                     duplicate_candidate_count = candidate_counts.duplicate_candidate_count,
                     review_candidate_count = candidate_counts.review_candidate_count,
                     state = CASE
                         WHEN ia.state IN ('analyzed', 'review_required') THEN
                             CASE
                                 WHEN candidate_counts.review_candidate_count > 0 THEN 'review_required'
                                 ELSE 'analyzed'
                             END
                         ELSE ia.state
                     END,
                     analysis_completed_at = CASE
                         WHEN ia.state IN ('analyzed', 'review_required')
                             THEN COALESCE(ia.analysis_completed_at, now())
                         ELSE ia.analysis_completed_at
                     END,
                     updated_at = now()
                 FROM counts, candidate_counts
                 WHERE ia.id = $1
                 RETURNING ia.id, ia.import_run_id, ia.source_name, ia.source_path, ia.state,
                           ia.image_count, ia.fingerprinted_count,
                           ia.duplicate_candidate_count, ia.review_candidate_count,
                           ia.last_error_message, ia.analysis_started_at, ia.analysis_completed_at",
                &[&album_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to refresh album summary: {e}")))?;
        Ok(Self::album_status_from_row(&row))
    }

    /// Finalize a successfully analyzed album. Unlike review/counter refresh,
    /// this is the only path allowed to advance `analyzing` to an analysis-
    /// complete state. The guarded update prevents cancellation, retry, or a
    /// concurrent failure from being overwritten by a late checkpoint.
    pub async fn finalize_import_album_analysis(
        client: &Client,
        album_id: Uuid,
    ) -> Result<ImportAlbumStatus, AppError> {
        Self::refresh_album_workflow_summary(client, album_id).await?;
        let row = client
            .query_opt(
                "UPDATE import_albums ia
                 SET state = CASE
                         WHEN EXISTS (
                             SELECT 1
                             FROM duplicate_candidates dc
                             JOIN import_images si ON si.id = dc.source_image_id
                             LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                             WHERE si.import_album_id = ia.id
                               AND dc.decision IS NULL
                               AND rd.id IS NULL
                         ) THEN 'review_required'
                         ELSE 'analyzed'
                     END,
                     analysis_completed_at = now(),
                     updated_at = now()
                 WHERE ia.id = $1
                   AND ia.state = 'analyzing'
                 RETURNING ia.id, ia.import_run_id, ia.source_name, ia.source_path, ia.state,
                           ia.image_count, ia.fingerprinted_count,
                           ia.duplicate_candidate_count, ia.review_candidate_count,
                           ia.last_error_message, ia.analysis_started_at, ia.analysis_completed_at",
                &[&album_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to finalize album analysis: {e}")))?
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "album {album_id} is no longer analyzing; refusing to finalize a stale checkpoint"
                ))
            })?;
        Ok(Self::album_status_from_row(&row))
    }

    pub async fn refresh_review_album_and_run(
        client: &Client,
        album_id: Uuid,
    ) -> Result<ImportAlbumStatus, AppError> {
        let status = Self::refresh_album_workflow_summary(client, album_id).await?;
        let import_run_id = Uuid::parse_str(&status.import_run_id).map_err(|e| {
            AppError::Internal(format!(
                "failed to parse refreshed album import_run_id '{}': {e}",
                status.import_run_id
            ))
        })?;
        Self::refresh_import_run_statistics(client, import_run_id).await?;
        Ok(status)
    }

    pub async fn refresh_group_review_summaries(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<(), AppError> {
        client
            .execute(
                "WITH album_groups AS (
                    SELECT ia.id AS album_id,
                           COUNT(DISTINCT rg.id) FILTER (
                               WHERE rg.requires_manual_review AND rg.state = 'pending'
                           )::INTEGER AS pending_group_count
                    FROM import_albums ia
                    LEFT JOIN import_images ii ON ii.import_album_id = ia.id
                    LEFT JOIN review_group_members rgm
                      ON rgm.image_source = 'import' AND rgm.image_id = ii.id
                    LEFT JOIN review_groups rg ON rg.id = rgm.group_id
                    WHERE ia.import_run_id = $1
                    GROUP BY ia.id
                 )
                 UPDATE import_albums ia
                 SET review_candidate_count = album_groups.pending_group_count,
                     state = CASE
                         WHEN ia.state IN ('analyzed', 'review_required') THEN
                             CASE WHEN album_groups.pending_group_count > 0
                                  THEN 'review_required' ELSE 'analyzed' END
                         ELSE ia.state
                     END,
                     updated_at = now()
                 FROM album_groups
                 WHERE ia.id = album_groups.album_id",
                &[&import_run_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to refresh group review summaries: {e}"))
            })?;
        Self::refresh_import_run_statistics(client, import_run_id).await?;
        Ok(())
    }

    pub async fn reset_failed_album_for_retry(
        client: &Client,
        album_id: Uuid,
    ) -> Result<(), AppError> {
        client
            .batch_execute("BEGIN")
            .await
            .map_err(|e| AppError::Internal(format!("failed to begin album retry reset: {e}")))?;

        let result: Result<(), AppError> = async {
            let album = client
                .query_opt(
                    "SELECT state, import_run_id
                     FROM import_albums
                     WHERE id = $1
                     FOR UPDATE",
                    &[&album_id],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to lock album for retry: {e}"))
                })?
                .ok_or_else(|| AppError::Internal(format!("album {album_id} was not found")))?;
            let current_state: String = album.get("state");
            let import_run_id: Uuid = album.get("import_run_id");
            if current_state != ImportAlbumState::Failed.to_string() {
                return Err(AppError::Internal(format!(
                    "album {album_id} cannot be retried from state '{current_state}'; expected 'failed'"
                )));
            }

            let has_commit_evidence: bool = client
                .query_one(
                    "SELECT EXISTS (
                         SELECT 1 FROM import_plan_albums WHERE import_album_id = $1
                     ) OR EXISTS (
                         SELECT 1 FROM file_transactions WHERE import_album_id = $1
                     )",
                    &[&album_id],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to check album retry safety: {e}"))
                })?
                .get(0);
            if has_commit_evidence {
                return Err(AppError::Internal(format!(
                    "album {album_id} cannot be retried because an import plan or file transaction references it"
                )));
            }

            let affected_album_rows = client
                .query(
                    "SELECT DISTINCT source_album.id
                     FROM duplicate_candidates dc
                     JOIN import_images source_image ON source_image.id = dc.source_image_id
                     JOIN import_albums source_album ON source_album.id = source_image.import_album_id
                     WHERE source_album.id <> $1
                       AND source_album.state IN ('analyzed', 'review_required')
                       AND (
                           dc.source_image_id IN (
                               SELECT id FROM import_images WHERE import_album_id = $1
                           )
                           OR dc.candidate_source_image_id IN (
                               SELECT id FROM import_images WHERE import_album_id = $1
                           )
                       )",
                    &[&album_id],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!(
                        "failed to query album summaries affected by retry: {e}"
                    ))
                })?;
            let affected_album_ids: Vec<Uuid> = affected_album_rows
                .iter()
                .map(|row| row.get("id"))
                .collect();

            client
                .execute(
                    "DELETE FROM duplicate_candidates dc
                 WHERE dc.source_image_id IN (
                     SELECT id FROM import_images WHERE import_album_id = $1
                 )
                 OR dc.candidate_source_image_id IN (
                     SELECT id FROM import_images WHERE import_album_id = $1
                 )",
                    &[&album_id],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to delete album candidates: {e}"))
                })?;
            client
                .execute(
                    "DELETE FROM import_images WHERE import_album_id = $1",
                    &[&album_id],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to delete album images: {e}")))?;
            let state = ImportAlbumState::Pending.to_string();
            client
                .execute(
                    "UPDATE import_albums
                 SET state = $1,
                     analysis_started_at = NULL,
                     analysis_completed_at = NULL,
                     last_error_code = NULL,
                     last_error_message = NULL,
                     image_count = 0,
                     fingerprinted_count = 0,
                     duplicate_candidate_count = 0,
                     review_candidate_count = 0,
                     updated_at = now()
                 WHERE id = $2",
                    &[&state, &album_id],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to reset album: {e}")))?;

            for affected_album_id in affected_album_ids {
                Self::refresh_album_workflow_summary(client, affected_album_id).await?;
            }
            Self::refresh_import_run_statistics(client, import_run_id).await?;
            Ok(())
        }
        .await;

        if let Err(e) = result {
            let _ = client.batch_execute("ROLLBACK").await;
            return Err(e);
        }

        client
            .batch_execute("COMMIT")
            .await
            .map_err(|e| AppError::Internal(format!("failed to commit album retry reset: {e}")))?;
        Ok(())
    }

    pub async fn insert_import_image(
        client: &Client,
        new_image: NewImportImage,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        let ds = new_image.decode_state.to_string();
        let st = new_image.state.to_string();
        client
            .execute(
                "INSERT INTO import_images
                 (id, import_album_id, source_path, relative_path, file_size, modified_at,
                  width, height, format, decode_state, blake3, pixel_hash,
                  block_hash_16, double_gradient_hash_32,
                  perceptual_eligible, fingerprint_version, state)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
                &[
                    &id,
                    &new_image.album_id,
                    &new_image.source_path,
                    &new_image.relative_path,
                    &new_image.file_size,
                    &new_image.modified_at,
                    &new_image.width,
                    &new_image.height,
                    &new_image.format,
                    &ds,
                    &new_image.blake3,
                    &new_image.pixel_hash,
                    &new_image.block_hash_16,
                    &new_image.double_gradient_hash_32,
                    &new_image.perceptual_eligible,
                    &new_image.fingerprint_version,
                    &st,
                ],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert import image: {e}")))?;
        Ok(id)
    }

    /// Persist an album checkpoint in one PostgreSQL round trip. IDs are
    /// allocated by the caller so duplicate detection can retain the mapping
    /// without reading every row back individually.
    pub async fn insert_import_images_batch(
        client: &Client,
        images: &[(Uuid, NewImportImage)],
    ) -> Result<(), AppError> {
        if images.is_empty() {
            return Ok(());
        }
        let ids: Vec<Uuid> = images.iter().map(|(id, _)| *id).collect();
        let album_ids: Vec<Uuid> = images.iter().map(|(_, image)| image.album_id).collect();
        let source_paths: Vec<String> = images
            .iter()
            .map(|(_, image)| image.source_path.clone())
            .collect();
        let relative_paths: Vec<String> = images
            .iter()
            .map(|(_, image)| image.relative_path.clone())
            .collect();
        let file_sizes: Vec<i64> = images.iter().map(|(_, image)| image.file_size).collect();
        let modified_at: Vec<Option<chrono::DateTime<chrono::Utc>>> =
            images.iter().map(|(_, image)| image.modified_at).collect();
        let widths: Vec<Option<i32>> = images.iter().map(|(_, image)| image.width).collect();
        let heights: Vec<Option<i32>> = images.iter().map(|(_, image)| image.height).collect();
        let formats: Vec<Option<String>> = images
            .iter()
            .map(|(_, image)| image.format.clone())
            .collect();
        let decode_states: Vec<String> = images
            .iter()
            .map(|(_, image)| image.decode_state.to_string())
            .collect();
        let blake3: Vec<Option<Vec<u8>>> = images
            .iter()
            .map(|(_, image)| image.blake3.clone())
            .collect();
        let pixel_hashes: Vec<Option<Vec<u8>>> = images
            .iter()
            .map(|(_, image)| image.pixel_hash.clone())
            .collect();
        let block_hashes: Vec<Option<Vec<u8>>> = images
            .iter()
            .map(|(_, image)| image.block_hash_16.clone())
            .collect();
        let double_gradient_hashes: Vec<Option<Vec<u8>>> = images
            .iter()
            .map(|(_, image)| image.double_gradient_hash_32.clone())
            .collect();
        let perceptual_eligibility: Vec<bool> = images
            .iter()
            .map(|(_, image)| image.perceptual_eligible)
            .collect();
        let fingerprint_versions: Vec<Option<String>> = images
            .iter()
            .map(|(_, image)| image.fingerprint_version.clone())
            .collect();
        let states: Vec<String> = images
            .iter()
            .map(|(_, image)| image.state.to_string())
            .collect();
        client
            .execute(
                "INSERT INTO import_images
                    (id, import_album_id, source_path, relative_path, file_size, modified_at,
                     width, height, format, decode_state, blake3, pixel_hash,
                     block_hash_16, double_gradient_hash_32, perceptual_eligible,
                     fingerprint_version, state)
                 SELECT * FROM UNNEST(
                    $1::uuid[], $2::uuid[], $3::text[], $4::text[],
                    $5::bigint[], $6::timestamptz[], $7::integer[], $8::integer[],
                    $9::text[], $10::text[], $11::bytea[], $12::bytea[],
                    $13::bytea[], $14::bytea[], $15::boolean[], $16::text[], $17::text[])",
                &[
                    &ids,
                    &album_ids,
                    &source_paths,
                    &relative_paths,
                    &file_sizes,
                    &modified_at,
                    &widths,
                    &heights,
                    &formats,
                    &decode_states,
                    &blake3,
                    &pixel_hashes,
                    &block_hashes,
                    &double_gradient_hashes,
                    &perceptual_eligibility,
                    &fingerprint_versions,
                    &states,
                ],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to batch insert import images: {e}"))
            })?;
        Ok(())
    }

    pub async fn get_import_images_by_album(
        client: &Client,
        album_id: Uuid,
    ) -> Result<Vec<ImportImageRecord>, AppError> {
        let rows = client
            .query(
                "SELECT id, source_path, relative_path, file_size, modified_at,
                        width, height, format, decode_state, blake3, pixel_hash,
                        fingerprint_version, state
                 FROM import_images WHERE import_album_id = $1",
                &[&album_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to get images by album: {e}")))?;

        Ok(rows
            .iter()
            .map(|r| ImportImageRecord {
                id: r.get("id"),
                source_path: r.get("source_path"),
                relative_path: r.get("relative_path"),
                file_size: r.get("file_size"),
                modified_at: r.get("modified_at"),
                width: r.get("width"),
                height: r.get("height"),
                format: r.get("format"),
                decode_state: r.get("decode_state"),
                blake3: r.get("blake3"),
                pixel_hash: r.get("pixel_hash"),
                fingerprint_version: r.get("fingerprint_version"),
                state: r.get("state"),
            })
            .collect())
    }

    pub async fn insert_duplicate_candidate(
        client: &Client,
        candidate: NewDuplicateCandidate,
    ) -> Result<Uuid, AppError> {
        let (id, _) = Self::upsert_duplicate_candidate(client, candidate).await?;
        Ok(id)
    }

    /// Insert one logical image pair, or merge stronger evidence into the
    /// existing row. The database unique indexes are the final concurrency
    /// guard; the returned bool tells progress accounting whether a row was
    /// actually added.
    pub async fn upsert_duplicate_candidate(
        client: &Client,
        candidate: NewDuplicateCandidate,
    ) -> Result<(Uuid, bool), AppError> {
        let id = Uuid::new_v4();
        let scope_str = candidate.scope.to_string();
        let match_str = candidate.match_type.to_string();
        let decision_str = candidate.decision.as_ref().map(|d| d.to_string());
        let source_str = candidate.decision_source.as_ref().map(|s| s.to_string());
        let inserted = client
            .query_opt(
                "INSERT INTO duplicate_candidates
                 (id, import_run_id, source_image_id, candidate_source_image_id,
                 candidate_library_image_id, scope, match_type,
                  blake3_equal, pixel_hash_equal,
                  block_distance, double_gradient_distance,
                  block_distance_ratio, double_gradient_distance_ratio,
                  transform_type, confidence,
                  decision, decision_source, rule_version)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
                 ON CONFLICT DO NOTHING
                 RETURNING id",
                &[
                    &id,
                    &candidate.import_run_id,
                    &candidate.source_image_id,
                    &candidate.candidate_source_image_id,
                    &candidate.candidate_library_image_id,
                    &scope_str,
                    &match_str,
                    &candidate.blake3_equal,
                    &candidate.pixel_hash_equal,
                    &candidate.block_distance,
                    &candidate.double_gradient_distance,
                    &candidate.block_distance_ratio,
                    &candidate.double_gradient_distance_ratio,
                    &candidate.transform_type,
                    &candidate.confidence,
                    &decision_str,
                    &source_str,
                    &SCAN_POLICY_VERSION.to_string(),
                ],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to insert duplicate candidate: {e}"))
            })?;
        if inserted.is_some() {
            return Ok((id, true));
        }

        let existing = if let Some(candidate_source_id) = candidate.candidate_source_image_id {
            client
                .query_one(
                    "SELECT id, match_type FROM duplicate_candidates
                     WHERE import_run_id = $1
                       AND candidate_source_image_id IS NOT NULL
                       AND LEAST(source_image_id, candidate_source_image_id) = LEAST($2::uuid, $3::uuid)
                       AND GREATEST(source_image_id, candidate_source_image_id) = GREATEST($2::uuid, $3::uuid)",
                    &[&candidate.import_run_id, &candidate.source_image_id, &candidate_source_id],
                )
                .await
        } else if let Some(candidate_library_id) = candidate.candidate_library_image_id {
            client
                .query_one(
                    "SELECT id, match_type FROM duplicate_candidates
                     WHERE import_run_id = $1 AND source_image_id = $2
                       AND candidate_library_image_id = $3",
                    &[&candidate.import_run_id, &candidate.source_image_id, &candidate_library_id],
                )
                .await
        } else {
            return Err(AppError::Internal(
                "duplicate candidate has no candidate image".to_string(),
            ));
        }
        .map_err(|e| AppError::Internal(format!("failed to load conflicting candidate: {e}")))?;
        let existing_id: Uuid = existing.get("id");
        let existing_match: String = existing.get("match_type");
        let priority = |value: &str| match value {
            "file_exact" => 0,
            "pixel_exact" => 1,
            "perceptual_near" => 2,
            _ => 3,
        };
        if priority(&match_str) < priority(&existing_match) {
            client
                .execute(
                    "UPDATE duplicate_candidates
                     SET scope = $2, match_type = $3,
                         blake3_equal = $4, pixel_hash_equal = $5,
                         block_distance = $6, double_gradient_distance = $7,
                         block_distance_ratio = $8, double_gradient_distance_ratio = $9,
                         transform_type = $10, confidence = $11,
                         decision = $12, decision_source = $13, rule_version = $14
                     WHERE id = $1",
                    &[
                        &existing_id,
                        &scope_str,
                        &match_str,
                        &candidate.blake3_equal,
                        &candidate.pixel_hash_equal,
                        &candidate.block_distance,
                        &candidate.double_gradient_distance,
                        &candidate.block_distance_ratio,
                        &candidate.double_gradient_distance_ratio,
                        &candidate.transform_type,
                        &candidate.confidence,
                        &decision_str,
                        &source_str,
                        &SCAN_POLICY_VERSION.to_string(),
                    ],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to merge candidate evidence: {e}"))
                })?;
        }
        Ok((existing_id, false))
    }

    /// Persist a complete album's deduplicated candidate set in one database
    /// round trip. Callers canonicalize import/import pairs before this point;
    /// the unique indexes remain the concurrency and retry guard.
    pub async fn upsert_duplicate_candidates_batch(
        client: &Client,
        candidates: &[NewDuplicateCandidate],
    ) -> Result<u64, AppError> {
        if candidates.is_empty() {
            return Ok(0);
        }
        let payload = serde_json::Value::Array(
            candidates
                .iter()
                .map(|candidate| {
                    serde_json::json!({
                        "id": Uuid::new_v4(),
                        "import_run_id": candidate.import_run_id,
                        "source_image_id": candidate.source_image_id,
                        "candidate_source_image_id": candidate.candidate_source_image_id,
                        "candidate_library_image_id": candidate.candidate_library_image_id,
                        "scope": candidate.scope.to_string(),
                        "match_type": candidate.match_type.to_string(),
                        "blake3_equal": candidate.blake3_equal,
                        "pixel_hash_equal": candidate.pixel_hash_equal,
                        "block_distance": candidate.block_distance,
                        "double_gradient_distance": candidate.double_gradient_distance,
                        "block_distance_ratio": candidate.block_distance_ratio,
                        "double_gradient_distance_ratio": candidate.double_gradient_distance_ratio,
                        "transform_type": candidate.transform_type,
                        "confidence": candidate.confidence,
                        "decision": candidate.decision.as_ref().map(ToString::to_string),
                        "decision_source": candidate.decision_source.as_ref().map(ToString::to_string),
                        "rule_version": SCAN_POLICY_VERSION,
                    })
                })
                .collect(),
        );
        let rows = client
            .query(
                "INSERT INTO duplicate_candidates
                    (id, import_run_id, source_image_id, candidate_source_image_id,
                     candidate_library_image_id, scope, match_type,
                     blake3_equal, pixel_hash_equal,
                     block_distance, double_gradient_distance,
                     block_distance_ratio, double_gradient_distance_ratio,
                     transform_type, confidence, decision, decision_source, rule_version)
                 SELECT x.id, x.import_run_id, x.source_image_id, x.candidate_source_image_id,
                        x.candidate_library_image_id, x.scope, x.match_type,
                        x.blake3_equal, x.pixel_hash_equal,
                        x.block_distance, x.double_gradient_distance,
                        x.block_distance_ratio, x.double_gradient_distance_ratio,
                        x.transform_type, x.confidence, x.decision, x.decision_source,
                        x.rule_version
                 FROM jsonb_to_recordset($1) AS x(
                    id uuid, import_run_id uuid, source_image_id uuid,
                    candidate_source_image_id uuid, candidate_library_image_id uuid,
                    scope text, match_type text, blake3_equal boolean,
                    pixel_hash_equal boolean, block_distance integer,
                    double_gradient_distance integer, block_distance_ratio double precision,
                    double_gradient_distance_ratio double precision, transform_type text,
                    confidence double precision, decision text, decision_source text,
                    rule_version text)
                 ON CONFLICT DO NOTHING
                 RETURNING id",
                &[&payload],
            )
            .await
            .map_err(|error| {
                AppError::Internal(format!(
                    "failed to batch upsert duplicate candidates: {error}"
                ))
            })?;
        Ok(rows.len() as u64)
    }

    pub async fn get_library_images_for_comparison(
        client: &Client,
    ) -> Result<Vec<LibraryImageRow>, AppError> {
        let rows = client
            .query(
                "SELECT id, file_size, blake3, pixel_hash, block_hash_16,
                        double_gradient_hash_32, perceptual_eligible, fingerprint_version
                 FROM library_images
                 WHERE fingerprint_version = '2' AND block_hash_16 IS NOT NULL
                 ORDER BY id",
                &[],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query library images: {e}")))?;

        Ok(rows
            .iter()
            .map(|r| LibraryImageRow {
                id: r.get("id"),
                file_size: r.get("file_size"),
                blake3: r.get("blake3"),
                pixel_hash: r.get("pixel_hash"),
                block_hash_16: r.get("block_hash_16"),
                double_gradient_hash_32: r.get("double_gradient_hash_32"),
                perceptual_eligible: r.get("perceptual_eligible"),
                fingerprint_version: r.get("fingerprint_version"),
            })
            .collect())
    }

    /// Batch query library images by BLAKE3 hashes (indexed exact match).
    pub async fn find_library_images_by_blake3(
        client: &Client,
        blake3_hashes: &[Vec<u8>],
    ) -> Result<Vec<LibraryImageRow>, AppError> {
        if blake3_hashes.is_empty() {
            return Ok(Vec::new());
        }
        let rows = client
            .query(
                "SELECT id, file_size, blake3, pixel_hash, block_hash_16,
                        double_gradient_hash_32, perceptual_eligible, fingerprint_version
                 FROM library_images
                 WHERE fingerprint_version = '2' AND blake3 = ANY($1)
                 ORDER BY id",
                &[&blake3_hashes],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query library by blake3: {e}")))?;

        Ok(rows
            .iter()
            .map(|r| LibraryImageRow {
                id: r.get("id"),
                file_size: r.get("file_size"),
                blake3: r.get("blake3"),
                pixel_hash: r.get("pixel_hash"),
                block_hash_16: r.get("block_hash_16"),
                double_gradient_hash_32: r.get("double_gradient_hash_32"),
                perceptual_eligible: r.get("perceptual_eligible"),
                fingerprint_version: r.get("fingerprint_version"),
            })
            .collect())
    }

    /// Load the fine verification evidence for one recalled candidate set in
    /// a single query. The caller supplies a stable, capped UUID list.
    pub async fn find_library_images_by_ids(
        client: &Client,
        image_ids: &[Uuid],
    ) -> Result<Vec<LibraryImageRow>, AppError> {
        if image_ids.is_empty() {
            return Ok(Vec::new());
        }

        let rows = client
            .query(
                "SELECT id, file_size, blake3, pixel_hash, block_hash_16,
                        double_gradient_hash_32, perceptual_eligible, fingerprint_version
                 FROM library_images
                 WHERE fingerprint_version = '2' AND id = ANY($1)
                 ORDER BY id",
                &[&image_ids],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!(
                    "failed to batch query recalled library images: {e}"
                ))
            })?;

        Ok(rows
            .iter()
            .map(|r| LibraryImageRow {
                id: r.get("id"),
                file_size: r.get("file_size"),
                blake3: r.get("blake3"),
                pixel_hash: r.get("pixel_hash"),
                block_hash_16: r.get("block_hash_16"),
                double_gradient_hash_32: r.get("double_gradient_hash_32"),
                perceptual_eligible: r.get("perceptual_eligible"),
                fingerprint_version: r.get("fingerprint_version"),
            })
            .collect())
    }

    /// V2 rows written by a successful import run at or after the captured
    /// commit boundary. Used to avoid reloading the run's historical rows.
    pub async fn get_library_images_for_import_run_committed_after(
        client: &Client,
        import_run_id: Uuid,
        committed_after: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<LibraryImageRow>, AppError> {
        let rows = client
            .query(
                "SELECT li.id, li.file_size, li.blake3, li.pixel_hash,
                        li.block_hash_16, li.double_gradient_hash_32,
                        li.perceptual_eligible, li.fingerprint_version
                 FROM library_images li
                 JOIN library_albums la ON la.id = li.album_id
                 JOIN file_transactions ft ON ft.id = la.transaction_id
                 WHERE ft.import_run_id = $1 AND li.fingerprint_version = '2'
                   AND ($2::timestamptz IS NULL OR li.committed_at >= $2)
                 ORDER BY li.id",
                &[&import_run_id, &committed_after],
            )
            .await
            .map_err(|error| {
                AppError::Internal(format!(
                    "failed to query committed V2 fingerprints for index update: {error}"
                ))
            })?;
        Ok(rows
            .iter()
            .map(|row| LibraryImageRow {
                id: row.get("id"),
                file_size: row.get("file_size"),
                blake3: row.get("blake3"),
                pixel_hash: row.get("pixel_hash"),
                block_hash_16: row.get("block_hash_16"),
                double_gradient_hash_32: row.get("double_gradient_hash_32"),
                perceptual_eligible: row.get("perceptual_eligible"),
                fingerprint_version: row.get("fingerprint_version"),
            })
            .collect())
    }

    /// Load exact fingerprints from albums whose analysis checkpoint is
    /// durable. Resumed scans use these rows to restore the run-level stable
    /// representatives without retaining full fingerprint thumbnails.
    pub async fn get_analyzed_run_exact_representatives(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<RunExactFingerprintRow>, AppError> {
        let rows = client
            .query(
                "WITH eligible AS (
                    SELECT ii.id, ii.import_album_id, ii.file_size, ii.blake3, ii.pixel_hash
                    FROM import_images ii
                    JOIN import_albums ia ON ii.import_album_id = ia.id
                    WHERE ia.import_run_id = $1
                      AND ia.state IN ('analyzed', 'review_required')
                      AND ii.fingerprint_version = '2'
                      AND ii.blake3 IS NOT NULL
                      AND ii.pixel_hash IS NOT NULL
                 ), file_representatives AS (
                    SELECT DISTINCT ON (file_size, blake3)
                           id, import_album_id, file_size, blake3, pixel_hash
                    FROM eligible
                    ORDER BY file_size, blake3, id
                 ), pixel_representatives AS (
                    SELECT DISTINCT ON (pixel_hash)
                           id, import_album_id, file_size, blake3, pixel_hash
                    FROM eligible
                    ORDER BY pixel_hash, id
                 )
                 SELECT id, import_album_id, file_size, blake3, pixel_hash
                 FROM file_representatives
                 UNION
                 SELECT id, import_album_id, file_size, blake3, pixel_hash
                 FROM pixel_representatives
                 ORDER BY id",
                &[&import_run_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!(
                    "failed to load analyzed run exact representatives: {e}"
                ))
            })?;

        Ok(rows
            .iter()
            .map(|row| RunExactFingerprintRow {
                id: row.get("id"),
                album_id: row.get("import_album_id"),
                file_size: row.get("file_size"),
                blake3: row.get("blake3"),
                pixel_hash: row.get("pixel_hash"),
            })
            .collect())
    }

    pub async fn count_duplicates_for_run(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<i64, AppError> {
        let row = client
            .query_one(
                "SELECT COUNT(*) FROM duplicate_candidates WHERE import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to count duplicates: {e}")))?;
        Ok(row.get(0))
    }

    pub async fn get_review_candidates(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<ReviewCandidateRow>, AppError> {
        let rows = client
            .query(
                "SELECT dc.id AS candidate_id,
                        dc.source_image_id,
                        dc.candidate_source_image_id,
                        dc.candidate_library_image_id,
                        dc.scope,
                        dc.match_type,
                        dc.transform_type,
                        dc.confidence,
                        ia.id AS album_id,
                        ia.source_name AS album_name,
                        (rd.id IS NOT NULL) AS has_decision
                 FROM duplicate_candidates dc
                 JOIN import_images si ON dc.source_image_id = si.id
                 JOIN import_albums ia ON si.import_album_id = ia.id
                 LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                 WHERE dc.import_run_id = $1
                   AND dc.decision IS NULL
                 ORDER BY ia.source_name, dc.created_at",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query review candidates: {e}")))?;

        Ok(rows
            .iter()
            .map(|r| ReviewCandidateRow {
                candidate_id: r.get("candidate_id"),
                source_image_id: r.get("source_image_id"),
                candidate_source_image_id: r.get("candidate_source_image_id"),
                candidate_library_image_id: r.get("candidate_library_image_id"),
                scope: r.get("scope"),
                match_type: r.get("match_type"),
                transform_type: r.get("transform_type"),
                confidence: r.get("confidence"),
                album_name: r.get("album_name"),
                has_decision: r.get("has_decision"),
            })
            .collect())
    }

    pub async fn get_review_candidate_detail(
        client: &Client,
        candidate_id: Uuid,
    ) -> Result<Option<ReviewCandidateDetailRow>, AppError> {
        let row = client
            .query_opt(
                "SELECT dc.id AS candidate_id,
                        dc.source_image_id,
                        si.source_path AS source_image_path,
                        si.file_size AS source_image_file_size,
                        si.width AS source_image_width,
                        si.height AS source_image_height,
                        dc.candidate_source_image_id,
                        csi.source_path AS candidate_source_image_path,
                        csi.file_size AS candidate_source_image_file_size,
                        csi.width AS candidate_source_image_width,
                        csi.height AS candidate_source_image_height,
                        dc.candidate_library_image_id,
                        concat_ws('\\', lr.path, NULLIF(la.relative_path, ''), cli.relative_path)
                            AS candidate_library_image_path,
                        cli.file_size AS candidate_library_image_file_size,
                        cli.width AS candidate_library_image_width,
                        cli.height AS candidate_library_image_height,
                        dc.scope,
                        dc.match_type,
                        dc.blake3_equal,
                        dc.pixel_hash_equal,
                        dc.block_distance,
                        dc.double_gradient_distance,
                        dc.block_distance_ratio,
                        dc.double_gradient_distance_ratio,
                        dc.transform_type,
                        dc.confidence,
                        ia.source_name AS album_name,
                        ia.id AS album_id,
                        rd.decision AS existing_decision
                 FROM duplicate_candidates dc
                 JOIN import_images si ON dc.source_image_id = si.id
                 JOIN import_albums ia ON si.import_album_id = ia.id
                 LEFT JOIN import_images csi ON dc.candidate_source_image_id = csi.id
                 LEFT JOIN library_images cli ON dc.candidate_library_image_id = cli.id
                 LEFT JOIN library_albums la ON cli.album_id = la.id
                 LEFT JOIN library_roots lr ON la.library_root_id = lr.id
                 LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                 WHERE dc.id = $1",
                &[&candidate_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query review candidate detail: {e}"))
            })?;

        Ok(row.map(|r| ReviewCandidateDetailRow {
            candidate_id: r.get("candidate_id"),
            source_image_id: r.get("source_image_id"),
            source_image_path: r.get("source_image_path"),
            source_image_file_size: r.get("source_image_file_size"),
            source_image_width: r.get("source_image_width"),
            source_image_height: r.get("source_image_height"),
            candidate_source_image_id: r.get("candidate_source_image_id"),
            candidate_source_image_path: r.get("candidate_source_image_path"),
            candidate_source_image_file_size: r.get("candidate_source_image_file_size"),
            candidate_source_image_width: r.get("candidate_source_image_width"),
            candidate_source_image_height: r.get("candidate_source_image_height"),
            candidate_library_image_id: r.get("candidate_library_image_id"),
            candidate_library_image_path: r.get("candidate_library_image_path"),
            candidate_library_image_file_size: r.get("candidate_library_image_file_size"),
            candidate_library_image_width: r.get("candidate_library_image_width"),
            candidate_library_image_height: r.get("candidate_library_image_height"),
            scope: r.get("scope"),
            match_type: r.get("match_type"),
            blake3_equal: r.get("blake3_equal"),
            pixel_hash_equal: r.get("pixel_hash_equal"),
            block_distance: r.get("block_distance"),
            double_gradient_distance: r.get("double_gradient_distance"),
            block_distance_ratio: r.get("block_distance_ratio"),
            double_gradient_distance_ratio: r.get("double_gradient_distance_ratio"),
            transform_type: r.get("transform_type"),
            confidence: r.get("confidence"),
            album_name: r.get("album_name"),
            album_id: r.get("album_id"),
            existing_decision: r.get("existing_decision"),
        }))
    }

    pub async fn get_review_decision(
        client: &Client,
        candidate_id: Uuid,
    ) -> Result<Option<String>, AppError> {
        let row = client
            .query_opt(
                "SELECT decision FROM review_decisions WHERE candidate_id = $1",
                &[&candidate_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query review decision: {e}")))?;

        Ok(row.map(|r| r.get("decision")))
    }

    pub async fn insert_review_decision_once(
        client: &Client,
        candidate_id: Uuid,
        decision: &str,
        selected_image_id: Option<Uuid>,
        notes: Option<&str>,
    ) -> Result<(), AppError> {
        let id = Uuid::new_v4();
        let row = client
            .query_opt(
                "INSERT INTO review_decisions (id, candidate_id, decision, selected_image_id, notes)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (candidate_id) DO UPDATE
                 SET decision = review_decisions.decision
                 WHERE review_decisions.decision = EXCLUDED.decision
                   AND review_decisions.selected_image_id IS NOT DISTINCT FROM EXCLUDED.selected_image_id
                 RETURNING id",
                &[&id, &candidate_id, &decision, &selected_image_id, &notes],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to insert review decision: {e}"))
            })?;

        if row.is_none() {
            return Err(AppError::Internal(format!(
                "candidate {candidate_id} already has a conflicting review decision"
            )));
        }

        Ok(())
    }

    pub async fn get_review_progress(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<ReviewProgressRow, AppError> {
        let row = client
            .query_one(
                "SELECT COUNT(*)::BIGINT AS total,
                        COUNT(*) FILTER (WHERE state = 'resolved')::BIGINT AS decided
                 FROM review_groups
                 WHERE import_run_id = $1 AND requires_manual_review",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to count review groups: {e}")))?;
        let total: i64 = row.get("total");
        let decided: i64 = row.get("decided");

        Ok(ReviewProgressRow {
            total: total as u32,
            decided: decided as u32,
        })
    }

    pub async fn has_review_groups(client: &Client, import_run_id: Uuid) -> Result<bool, AppError> {
        client
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM review_groups WHERE import_run_id = $1)",
                &[&import_run_id],
            )
            .await
            .map(|row| row.get(0))
            .map_err(|e| AppError::Internal(format!("failed to query review groups: {e}")))
    }

    pub async fn get_review_groups(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<ReviewGroupSummaryRow>, AppError> {
        let rows = client
            .query(
                "SELECT rg.id AS group_id, rg.state, rg.requires_manual_review,
                        COUNT(rgm.id)::BIGINT AS member_count,
                        COUNT(rgm.id) FILTER (WHERE rgm.image_source = 'import')::BIGINT AS import_member_count,
                        COUNT(rgm.id) FILTER (WHERE rgm.image_source = 'library')::BIGINT AS library_member_count,
                        COUNT(rgm.id) FILTER (WHERE rgm.final_action = 'keep')::BIGINT AS kept_count
                 FROM review_groups rg
                 JOIN review_group_members rgm ON rgm.group_id = rg.id
                 WHERE rg.import_run_id = $1
                 GROUP BY rg.id
                 ORDER BY (rg.state = 'resolved'), rg.created_at, rg.id",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query review groups: {e}")))?;
        Ok(rows
            .iter()
            .map(|row| ReviewGroupSummaryRow {
                group_id: row.get("group_id"),
                state: row.get("state"),
                requires_manual_review: row.get("requires_manual_review"),
                member_count: row.get("member_count"),
                import_member_count: row.get("import_member_count"),
                library_member_count: row.get("library_member_count"),
                kept_count: row.get("kept_count"),
            })
            .collect())
    }

    pub async fn get_review_group_members(
        client: &Client,
        group_id: Uuid,
    ) -> Result<Vec<ReviewGroupMemberRow>, AppError> {
        let rows = client
            .query(
                "SELECT rgm.image_id, rgm.image_source, rgm.final_action, rgm.decision_source,
                        ii.source_path,
                        ii.relative_path,
                        ia.source_name AS album_name,
                        ii.file_size, ii.width, ii.height, ii.format
                 FROM review_group_members rgm
                 JOIN import_images ii ON rgm.image_source = 'import' AND ii.id = rgm.image_id
                 JOIN import_albums ia ON ia.id = ii.import_album_id
                 WHERE rgm.group_id = $1
                 UNION ALL
                 SELECT rgm.image_id, rgm.image_source, rgm.final_action, rgm.decision_source,
                        concat_ws('\\', lr.path, NULLIF(la.relative_path, ''), li.relative_path) AS source_path,
                        li.relative_path,
                        la.display_name AS album_name,
                        li.file_size, li.width, li.height, li.format
                 FROM review_group_members rgm
                 JOIN library_images li ON rgm.image_source = 'library' AND li.id = rgm.image_id
                 JOIN library_albums la ON la.id = li.album_id
                 JOIN library_roots lr ON lr.id = la.library_root_id
                 WHERE rgm.group_id = $1
                 ORDER BY image_source DESC, album_name, relative_path, image_id",
                &[&group_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query review group members: {e}")))?;
        Ok(rows
            .iter()
            .map(|row| ReviewGroupMemberRow {
                image_id: row.get("image_id"),
                image_source: row.get("image_source"),
                final_action: row.get("final_action"),
                decision_source: row.get("decision_source"),
                source_path: row.get("source_path"),
                relative_path: row.get("relative_path"),
                album_name: row.get("album_name"),
                file_size: row.get("file_size"),
                width: row.get("width"),
                height: row.get("height"),
                format: row.get("format"),
            })
            .collect())
    }

    pub async fn get_review_group_evidence(
        client: &Client,
        group_id: Uuid,
    ) -> Result<Vec<ReviewGroupEvidenceRow>, AppError> {
        let rows = client
            .query(
                "SELECT dc.id AS candidate_id, dc.source_image_id,
                        COALESCE(dc.candidate_source_image_id, dc.candidate_library_image_id) AS candidate_image_id,
                        CASE WHEN dc.candidate_library_image_id IS NULL THEN 'import' ELSE 'library' END AS candidate_image_source,
                        dc.scope, dc.match_type, dc.blake3_equal, dc.pixel_hash_equal,
                        dc.block_distance, dc.double_gradient_distance,
                        dc.block_distance_ratio, dc.double_gradient_distance_ratio,
                        dc.transform_type, dc.confidence,
                        COALESCE(dc.decision = 'auto_duplicate', FALSE) AS automatic
                 FROM duplicate_candidates dc
                 JOIN review_group_members source_member
                   ON source_member.group_id = $1
                  AND source_member.image_source = 'import'
                  AND source_member.image_id = dc.source_image_id
                 JOIN review_group_members candidate_member
                   ON candidate_member.group_id = $1
                  AND candidate_member.image_id = COALESCE(dc.candidate_source_image_id, dc.candidate_library_image_id)
                  AND candidate_member.image_source = CASE
                        WHEN dc.candidate_library_image_id IS NULL THEN 'import' ELSE 'library' END
                 ORDER BY dc.created_at, dc.id",
                &[&group_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query review group evidence: {e}")))?;
        Ok(rows
            .iter()
            .map(|row| ReviewGroupEvidenceRow {
                candidate_id: row.get("candidate_id"),
                source_image_id: row.get("source_image_id"),
                candidate_image_id: row.get("candidate_image_id"),
                candidate_image_source: row.get("candidate_image_source"),
                scope: row.get("scope"),
                match_type: row.get("match_type"),
                blake3_equal: row.get("blake3_equal"),
                pixel_hash_equal: row.get("pixel_hash_equal"),
                block_distance: row.get("block_distance"),
                double_gradient_distance: row.get("double_gradient_distance"),
                block_distance_ratio: row.get("block_distance_ratio"),
                double_gradient_distance_ratio: row.get("double_gradient_distance_ratio"),
                transform_type: row.get("transform_type"),
                confidence: row.get("confidence"),
                automatic: row.get("automatic"),
            })
            .collect())
    }

    pub async fn get_review_group_excluded_import_ids(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<HashSet<Uuid>, AppError> {
        let rows = client
            .query(
                "SELECT rgm.image_id
                 FROM review_group_members rgm
                 JOIN review_groups rg ON rg.id = rgm.group_id
                 WHERE rg.import_run_id = $1
                   AND rgm.image_source = 'import'
                   AND rgm.final_action = 'exclude'",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to load group exclusions: {e}")))?;
        Ok(rows.iter().map(|row| row.get("image_id")).collect())
    }

    pub async fn get_all_candidates_for_import_plan(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<ImportPlanCandidateRow>, AppError> {
        let rows = client
            .query(
                "SELECT dc.id AS candidate_id,
                        dc.source_image_id,
                        dc.candidate_source_image_id,
                        dc.candidate_library_image_id,
                        dc.scope,
                        dc.decision AS candidate_decision,
                        rd.decision AS review_decision,
                        si.import_album_id AS source_album_id,
                        dc.blake3_equal,
                        dc.pixel_hash_equal,
                        dc.confidence
                 FROM duplicate_candidates dc
                 JOIN import_images si ON dc.source_image_id = si.id
                 JOIN import_albums source_album ON source_album.id = si.import_album_id
                 LEFT JOIN import_images csi ON csi.id = dc.candidate_source_image_id
                 LEFT JOIN import_albums candidate_album ON candidate_album.id = csi.import_album_id
                 LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                 WHERE dc.import_run_id = $1
                   AND source_album.state IN ('analyzed', 'review_required')
                   AND (
                       dc.candidate_source_image_id IS NULL
                       OR candidate_album.state IN ('analyzed', 'review_required')
                   )",
                &[&import_run_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query candidates for import plan: {e}"))
            })?;

        Ok(rows
            .iter()
            .map(|r| ImportPlanCandidateRow {
                candidate_id: r.get("candidate_id"),
                source_image_id: r.get("source_image_id"),
                candidate_source_image_id: r.get("candidate_source_image_id"),
                candidate_library_image_id: r.get("candidate_library_image_id"),
                scope: r.get("scope"),
                candidate_decision: r.get("candidate_decision"),
                review_decision: r.get("review_decision"),
                source_album_id: r.get("source_album_id"),
                blake3_equal: r.get("blake3_equal"),
                pixel_hash_equal: r.get("pixel_hash_equal"),
                confidence: r.get("confidence"),
            })
            .collect())
    }

    pub async fn get_all_import_images_with_album(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<ImportPlanImageRow>, AppError> {
        let rows = client
            .query(
                "SELECT ii.id,
                        ii.source_path,
                        ii.relative_path,
                        ii.file_size,
                        ia.id AS album_id,
                        ia.source_name AS album_name
                 FROM import_images ii
                 JOIN import_albums ia ON ii.import_album_id = ia.id
                 WHERE ia.import_run_id = $1
                   AND ii.state = 'fingerprinted'
                   AND ii.blake3 IS NOT NULL",
                &[&import_run_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query import images for plan: {e}"))
            })?;

        Ok(rows
            .iter()
            .map(|r| ImportPlanImageRow {
                id: r.get("id"),
                source_path: r.get("source_path"),
                relative_path: r.get("relative_path"),
                file_size: r.get("file_size"),
                album_id: r.get("album_id"),
                album_name: r.get("album_name"),
            })
            .collect())
    }

    pub async fn get_albums_for_run(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<AlbumRow>, AppError> {
        let rows = client
            .query(
                "SELECT id, source_name FROM import_albums WHERE import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query albums for run: {e}")))?;

        Ok(rows
            .iter()
            .map(|r| AlbumRow {
                id: r.get("id"),
                source_name: r.get("source_name"),
            })
            .collect())
    }

    fn album_status_from_row(row: &tokio_postgres::Row) -> ImportAlbumStatus {
        let started_at: Option<chrono::DateTime<chrono::Utc>> = row.get("analysis_started_at");
        let completed_at: Option<chrono::DateTime<chrono::Utc>> = row.get("analysis_completed_at");
        ImportAlbumStatus {
            id: row.get::<_, Uuid>("id").to_string(),
            import_run_id: row.get::<_, Uuid>("import_run_id").to_string(),
            source_name: row.get("source_name"),
            source_path: row.get("source_path"),
            state: row.get("state"),
            image_count: row.get("image_count"),
            fingerprinted_count: row.get("fingerprinted_count"),
            duplicate_candidate_count: row.get("duplicate_candidate_count"),
            review_candidate_count: row.get("review_candidate_count"),
            last_error_message: row.get("last_error_message"),
            analysis_started_at: started_at.map(|ts| ts.to_rfc3339()),
            analysis_completed_at: completed_at.map(|ts| ts.to_rfc3339()),
        }
    }

    pub async fn get_import_run_album_status(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<ImportAlbumStatus>, AppError> {
        let rows = client
            .query(
                "SELECT id, import_run_id, source_name, source_path, state,
                        image_count, fingerprinted_count,
                        duplicate_candidate_count, review_candidate_count,
                        last_error_message, analysis_started_at, analysis_completed_at
                 FROM import_albums
                 WHERE import_run_id = $1
                 ORDER BY source_name",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query album status: {e}")))?;
        Ok(rows.iter().map(Self::album_status_from_row).collect())
    }

    pub async fn get_import_album_status_by_id(
        client: &Client,
        album_id: Uuid,
    ) -> Result<Option<ImportAlbumStatus>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, import_run_id, source_name, source_path, state,
                        image_count, fingerprinted_count,
                        duplicate_candidate_count, review_candidate_count,
                        last_error_message, analysis_started_at, analysis_completed_at
                 FROM import_albums
                 WHERE id = $1",
                &[&album_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query album status: {e}")))?;
        Ok(row.map(|row| Self::album_status_from_row(&row)))
    }

    pub async fn list_resume_candidates(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<ImportAlbumStatus>, AppError> {
        let rows = client
            .query(
                "SELECT id, import_run_id, source_name, source_path, state,
                        image_count, fingerprinted_count,
                        duplicate_candidate_count, review_candidate_count,
                        last_error_message, analysis_started_at, analysis_completed_at
                 FROM import_albums
                 WHERE import_run_id = $1
                   AND state IN ('pending', 'analyzing', 'scanning', 'fingerprinting', 'failed')
                 ORDER BY source_name",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query resume candidates: {e}")))?;
        Ok(rows.iter().map(Self::album_status_from_row).collect())
    }

    pub async fn mark_stale_analyzing_albums(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<u64, AppError> {
        client
            .batch_execute("BEGIN")
            .await
            .map_err(|e| AppError::Internal(format!("failed to begin stale album cleanup: {e}")))?;

        let result: Result<u64, AppError> = async {
            let stale_rows = client
                .query(
                    "SELECT id
                     FROM import_albums
                     WHERE import_run_id = $1
                       AND state IN ('analyzing', 'scanning', 'fingerprinting')
                     FOR UPDATE",
                    &[&import_run_id],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to query stale albums: {e}")))?;
            let stale_album_ids: Vec<Uuid> = stale_rows.iter().map(|row| row.get("id")).collect();
            if stale_album_ids.is_empty() {
                return Ok(0);
            }

            let has_commit_evidence: bool = client
                .query_one(
                    "SELECT EXISTS (
                         SELECT 1
                         FROM import_plan_albums
                         WHERE import_album_id = ANY($1)
                     ) OR EXISTS (
                         SELECT 1
                         FROM file_transactions
                         WHERE import_album_id = ANY($1)
                     )",
                    &[&stale_album_ids],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to check stale album safety: {e}"))
                })?
                .get(0);
            if has_commit_evidence {
                return Err(AppError::Internal(
                    "cannot reset stale albums referenced by an import plan or file transaction"
                        .to_string(),
                ));
            }

            let affected_album_rows = client
                .query(
                    "SELECT DISTINCT source_album.id
                     FROM duplicate_candidates dc
                     JOIN import_images source_image ON source_image.id = dc.source_image_id
                     JOIN import_albums source_album ON source_album.id = source_image.import_album_id
                     WHERE NOT (source_album.id = ANY($1))
                       AND source_album.state IN ('analyzed', 'review_required')
                       AND dc.candidate_source_image_id IN (
                           SELECT id FROM import_images WHERE import_album_id = ANY($1)
                       )",
                    &[&stale_album_ids],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!(
                        "failed to query album summaries affected by stale cleanup: {e}"
                    ))
                })?;
            let affected_album_ids: Vec<Uuid> = affected_album_rows
                .iter()
                .map(|row| row.get("id"))
                .collect();

            client
                .execute(
                    "DELETE FROM duplicate_candidates dc
                     WHERE dc.source_image_id IN (
                         SELECT id FROM import_images WHERE import_album_id = ANY($1)
                     )
                     OR dc.candidate_source_image_id IN (
                         SELECT id FROM import_images WHERE import_album_id = ANY($1)
                     )",
                    &[&stale_album_ids],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to delete stale album candidates: {e}"))
                })?;
            client
                .execute(
                    "DELETE FROM import_images WHERE import_album_id = ANY($1)",
                    &[&stale_album_ids],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to delete stale album images: {e}"))
                })?;

            let pending = ImportAlbumState::Pending.to_string();
            let updated = client
                .execute(
                    "UPDATE import_albums
                     SET state = $1,
                         analysis_started_at = NULL,
                         analysis_completed_at = NULL,
                         last_error_code = NULL,
                         last_error_message = NULL,
                         image_count = 0,
                         fingerprinted_count = 0,
                         duplicate_candidate_count = 0,
                         review_candidate_count = 0,
                         updated_at = now()
                     WHERE id = ANY($2)",
                    &[&pending, &stale_album_ids],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to mark stale albums: {e}")))?;
            for affected_album_id in affected_album_ids {
                Self::refresh_album_workflow_summary(client, affected_album_id).await?;
            }
            Ok(updated)
        }
        .await;

        match result {
            Ok(updated) => {
                client.batch_execute("COMMIT").await.map_err(|e| {
                    AppError::Internal(format!("failed to commit stale album cleanup: {e}"))
                })?;
                Ok(updated)
            }
            Err(e) => {
                let _ = client.batch_execute("ROLLBACK").await;
                Err(e)
            }
        }
    }

    pub async fn list_import_runs_summary(
        client: &Client,
    ) -> Result<Vec<ImportRunDashboard>, AppError> {
        let rows = client
            .query(
                "SELECT r.id AS import_run_id,
                        r.source_root,
                        r.state,
                        COUNT(a.id)::INTEGER AS total_albums,
                        COUNT(a.id) FILTER (WHERE a.state = 'pending')::INTEGER AS pending_albums,
                        COUNT(a.id) FILTER (
                            WHERE a.state IN ('analyzing', 'scanning', 'fingerprinting')
                        )::INTEGER AS analyzing_albums,
                        COUNT(a.id) FILTER (WHERE a.state = 'analyzed')::INTEGER AS analyzed_albums,
                        COUNT(a.id) FILTER (WHERE a.state = 'review_required')::INTEGER AS review_required_albums,
                        COUNT(a.id) FILTER (WHERE a.state = 'failed')::INTEGER AS failed_albums,
                        COALESCE(SUM(a.image_count), 0)::INTEGER AS total_images,
                        COALESCE(SUM(a.review_candidate_count), 0)::INTEGER AS pending_reviews,
                        COALESCE(SUM(a.duplicate_candidate_count), 0)::INTEGER AS duplicate_candidates
                 FROM import_runs r
                 LEFT JOIN import_albums a ON a.import_run_id = r.id
                 GROUP BY r.id, r.source_root, r.state, r.started_at
                 ORDER BY r.started_at DESC
                 LIMIT 20",
                &[],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!(
                    "failed to query run dashboard: {}",
                    postgres_error_detail(&e)
                ))
            })?;

        Ok(rows
            .iter()
            .map(|r| ImportRunDashboard {
                import_run_id: r.get::<_, Uuid>("import_run_id").to_string(),
                source_root: r.get("source_root"),
                state: r.get("state"),
                total_albums: r.get("total_albums"),
                pending_albums: r.get("pending_albums"),
                analyzing_albums: r.get("analyzing_albums"),
                analyzed_albums: r.get("analyzed_albums"),
                review_required_albums: r.get("review_required_albums"),
                failed_albums: r.get("failed_albums"),
                total_images: r.get("total_images"),
                pending_reviews: r.get("pending_reviews"),
                duplicate_candidates: r.get("duplicate_candidates"),
            })
            .collect())
    }

    async fn get_latest_actionable_run_summary(
        client: &Client,
    ) -> Result<Option<DashboardActionableRun>, AppError> {
        let row = client.query_opt(
            "WITH run_facts AS (
               SELECT r.id AS import_run_id, r.source_root, r.state, r.started_at,
                    COUNT(a.id)::INTEGER AS total_albums,
                    COUNT(a.id) FILTER (WHERE a.state = 'pending')::INTEGER AS pending_albums,
                    COUNT(a.id) FILTER (WHERE a.state IN ('analyzing', 'scanning', 'fingerprinting'))::INTEGER AS analyzing_albums,
                    COUNT(a.id) FILTER (WHERE a.state = 'analyzed')::INTEGER AS analyzed_albums,
                    COUNT(a.id) FILTER (WHERE a.state = 'review_required')::INTEGER AS review_required_albums,
                    COUNT(a.id) FILTER (WHERE a.state = 'failed')::INTEGER AS failed_albums,
                    COALESCE(SUM(a.image_count), 0)::INTEGER AS total_images,
                    COALESCE(SUM(a.review_candidate_count), 0)::INTEGER AS pending_reviews,
                    COALESCE(SUM(a.duplicate_candidate_count), 0)::INTEGER AS duplicate_candidates,
                    EXISTS (
                        SELECT 1 FROM import_plans p
                        WHERE p.import_run_id = r.id
                          AND p.state IN ('frozen', 'consumed')
                          AND p.plan_hash IS NOT NULL
                    ) AS has_frozen_plan,
                    EXISTS (
                        SELECT 1 FROM file_transactions ft
                        WHERE ft.import_run_id = r.id
                          AND ft.state IN (
                              'planned', 'staging', 'verifying', 'verified',
                              'publishing', 'published', 'db_committing',
                              'library_committed', 'source_archiving', 'source_files_removing',
                              'cleanup_required', 'conflict'
                          )
                    ) AS has_recoverable_transaction,
                    EXISTS (
                        SELECT 1 FROM file_transactions ft
                        WHERE ft.import_run_id = r.id
                          AND ft.state IN ('failed', 'cancelled')
                    ) AS has_terminal_unresolved_transaction,
                    EXISTS (
                        SELECT 1
                        FROM import_plans p
                        JOIN import_plan_albums ipa ON ipa.plan_id = p.id
                        WHERE p.import_run_id = r.id
                          AND p.state IN ('frozen', 'consumed')
                          AND p.plan_hash IS NOT NULL
                          AND NOT EXISTS (
                              SELECT 1 FROM file_transactions ft
                              WHERE ft.import_run_id = r.id
                                AND ft.import_album_id = ipa.import_album_id
                          )
                    ) AS has_missing_plan_album_transaction
               FROM import_runs r
               LEFT JOIN import_albums a ON a.import_run_id = r.id
               WHERE r.state NOT IN ('abandoned', 'completed')
               GROUP BY r.id, r.source_root, r.state, r.started_at
             ), routed AS (
               SELECT *, CASE
                   WHEN has_recoverable_transaction THEN 'recover'
                   WHEN state = 'review_required' AND pending_reviews > 0 THEN 'review'
                   WHEN state = 'review_required' AND pending_reviews = 0 THEN 'generate_plan'
                   WHEN state = 'cancelled' AND has_frozen_plan
                        AND NOT has_terminal_unresolved_transaction THEN 'resume_commit'
                   WHEN state IN ('committing', 'recovery_required')
                        AND has_frozen_plan
                        AND has_missing_plan_album_transaction THEN 'resume_commit'
                   WHEN state IN ('committing', 'recovery_required')
                        AND has_terminal_unresolved_transaction THEN 'inspect_transaction_failure'
                   WHEN state IN ('committing', 'recovery_required')
                        AND has_frozen_plan THEN 'resume_commit'
                   WHEN state IN ('committing', 'recovery_required') THEN 'inspect_transaction_failure'
                   WHEN pending_albums > 0 OR analyzing_albums > 0 THEN 'resume_analysis'
                   WHEN state = 'failed' OR failed_albums > 0 THEN 'inspect_failed'
                   WHEN state = 'ready_to_commit' THEN 'generate_plan'
                   ELSE 'new_import'
               END AS next_action
               FROM run_facts
             )
             SELECT * FROM routed
             WHERE next_action <> 'new_import'
             ORDER BY CASE next_action
                        WHEN 'recover' THEN 0
                        WHEN 'inspect_transaction_failure' THEN 1
                        WHEN 'review' THEN 2
                        WHEN 'generate_plan' THEN 3
                        WHEN 'resume_analysis' THEN 4
                        WHEN 'inspect_failed' THEN 5
                        WHEN 'resume_commit' THEN 6
                        ELSE 7
                      END,
                      started_at DESC
             LIMIT 1",
            &[],
        ).await.map_err(|e| AppError::Internal(format!(
            "failed to query latest actionable run: {}",
            postgres_error_detail(&e)
        )))?;

        Ok(row.map(|r| {
            let next_action = match r.get::<_, String>("next_action").as_str() {
                "recover" => DashboardNextAction::Recover,
                "inspect_transaction_failure" => DashboardNextAction::InspectTransactionFailure,
                "review" => DashboardNextAction::Review,
                "generate_plan" => DashboardNextAction::GeneratePlan,
                "resume_analysis" => DashboardNextAction::ResumeAnalysis,
                "inspect_failed" => DashboardNextAction::InspectFailed,
                "resume_commit" => DashboardNextAction::ResumeCommit,
                _ => DashboardNextAction::NewImport,
            };
            DashboardActionableRun {
                run: ImportRunDashboard {
                    import_run_id: r.get::<_, Uuid>("import_run_id").to_string(),
                    source_root: r.get("source_root"),
                    state: r.get("state"),
                    total_albums: r.get("total_albums"),
                    pending_albums: r.get("pending_albums"),
                    analyzing_albums: r.get("analyzing_albums"),
                    analyzed_albums: r.get("analyzed_albums"),
                    review_required_albums: r.get("review_required_albums"),
                    failed_albums: r.get("failed_albums"),
                    total_images: r.get("total_images"),
                    pending_reviews: r.get("pending_reviews"),
                    duplicate_candidates: r.get("duplicate_candidates"),
                },
                next_action,
                has_frozen_plan: r.get("has_frozen_plan"),
                has_recoverable_transaction: r.get("has_recoverable_transaction"),
                has_terminal_unresolved_transaction: r.get("has_terminal_unresolved_transaction"),
                has_missing_plan_album_transaction: r.get("has_missing_plan_album_transaction"),
            }
        }))
    }

    pub async fn get_database_info_dashboard(
        client: &Client,
        database: DatabaseInfoDatabaseSection,
    ) -> Result<DatabaseInfoDashboard, AppError> {
        let library_row = client
            .query_one(
                "SELECT
                    (SELECT COUNT(*) FROM library_roots)::BIGINT AS library_root_count,
                    (SELECT COUNT(*) FROM library_albums)::BIGINT AS library_album_count,
                    (SELECT COUNT(*) FROM library_images)::BIGINT AS library_image_count",
                &[],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query library summary: {e}")))?;

        let imports_row = client
            .query_one(
                "SELECT
                    (SELECT COUNT(*) FROM import_runs)::BIGINT AS import_run_count,
                    (SELECT COUNT(*) FROM import_albums)::BIGINT AS import_album_count,
                    (SELECT COUNT(*) FROM import_images)::BIGINT AS import_image_count,
                    (
                        (SELECT COUNT(*)
                         FROM review_groups rg
                         JOIN import_runs r ON r.id = rg.import_run_id
                         WHERE r.state <> 'abandoned'
                           AND rg.requires_manual_review
                           AND rg.state = 'pending')
                        +
                        (SELECT COUNT(*)
                         FROM duplicate_candidates dc
                         JOIN import_runs r ON r.id = dc.import_run_id
                         LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                         WHERE r.state <> 'abandoned'
                           AND NOT EXISTS (
                               SELECT 1 FROM review_groups rg
                               WHERE rg.import_run_id = dc.import_run_id
                           )
                           AND dc.decision IS NULL AND rd.id IS NULL)
                    )::BIGINT AS pending_review_count,
                    (
                        SELECT COUNT(*) FROM import_albums a
                        JOIN import_runs r ON r.id = a.import_run_id
                        WHERE r.state <> 'abandoned' AND a.state = 'failed'
                    )::BIGINT AS failed_album_count,
                    (SELECT COUNT(*) FROM import_runs WHERE state = 'recovery_required')::BIGINT AS recovery_required_run_count,
                    (SELECT COUNT(*) FROM import_runs WHERE state = 'failed')::BIGINT AS failed_run_count,
                    (SELECT COUNT(*) FROM import_plans WHERE state = 'frozen')::BIGINT AS frozen_plan_count",
                &[],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query import summary: {e}")))?;

        let latest_run = Self::list_import_runs_summary(client)
            .await?
            .into_iter()
            .next();
        let latest_actionable_run = Self::get_latest_actionable_run_summary(client).await?;
        let next_action = latest_actionable_run
            .as_ref()
            .map(|run| run.next_action)
            .unwrap_or(DashboardNextAction::NewImport);

        Ok(DatabaseInfoDashboard {
            database,
            library: DatabaseInfoLibrarySection {
                library_root_count: library_row.get("library_root_count"),
                library_album_count: library_row.get("library_album_count"),
                library_image_count: library_row.get("library_image_count"),
            },
            imports: DatabaseInfoImportsSection {
                import_run_count: imports_row.get("import_run_count"),
                import_album_count: imports_row.get("import_album_count"),
                import_image_count: imports_row.get("import_image_count"),
                pending_review_count: imports_row.get("pending_review_count"),
                failed_album_count: imports_row.get("failed_album_count"),
                recovery_required_run_count: imports_row.get("recovery_required_run_count"),
                failed_run_count: imports_row.get("failed_run_count"),
                frozen_plan_count: imports_row.get("frozen_plan_count"),
            },
            latest_run,
            latest_actionable_run,
            next_action,
        })
    }

    pub fn empty_database_info_dashboard(
        database: DatabaseInfoDatabaseSection,
    ) -> DatabaseInfoDashboard {
        DatabaseInfoDashboard {
            database,
            library: DatabaseInfoLibrarySection {
                library_root_count: 0,
                library_album_count: 0,
                library_image_count: 0,
            },
            imports: DatabaseInfoImportsSection {
                import_run_count: 0,
                import_album_count: 0,
                import_image_count: 0,
                pending_review_count: 0,
                failed_album_count: 0,
                recovery_required_run_count: 0,
                failed_run_count: 0,
                frozen_plan_count: 0,
            },
            latest_run: None,
            latest_actionable_run: None,
            next_action: DashboardNextAction::NewImport,
        }
    }

    pub async fn get_latest_completed_run(client: &Client) -> Result<Option<Uuid>, AppError> {
        let row = client
            .query_opt(
                "SELECT id FROM import_runs
                 WHERE state = 'completed'
                 ORDER BY completed_at DESC NULLS LAST, started_at DESC
                 LIMIT 1",
                &[],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query latest run: {e}")))?;

        Ok(row.map(|r| r.get("id")))
    }

    /// Find the most recent run that the review page should load: a run in
    /// `review_required` (undecided candidates) or `ready_to_commit` (all
    /// decided, plan not yet committed). A freshly-finished scan leaves the
    /// run in one of these states — never `completed` — so the review page
    /// cannot use `get_latest_completed_run`. Picks the newest by
    /// `started_at` so a re-scan supersedes an older review run.
    pub async fn get_latest_reviewable_run(client: &Client) -> Result<Option<Uuid>, AppError> {
        let row = client
            .query_opt(
                "SELECT id FROM import_runs
                 WHERE state <> 'abandoned'
                   AND (state IN ('review_required', 'ready_to_commit')
                    OR EXISTS (
                        SELECT 1 FROM review_groups rg
                        WHERE rg.import_run_id = import_runs.id
                          AND rg.requires_manual_review
                          AND rg.state = 'pending'
                    )
                    OR EXISTS (
                        SELECT 1
                        FROM duplicate_candidates dc
                        JOIN import_images ii ON ii.id = dc.source_image_id
                        JOIN import_albums ia ON ia.id = ii.import_album_id
                        LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
                        WHERE ia.import_run_id = import_runs.id
                          AND NOT EXISTS (
                              SELECT 1 FROM review_groups rg
                              WHERE rg.import_run_id = import_runs.id
                          )
                          AND dc.decision IS NULL
                          AND rd.id IS NULL
                    ))
                 ORDER BY started_at DESC
                 LIMIT 1",
                &[],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query latest reviewable run: {e}"))
            })?;

        Ok(row.map(|r| r.get("id")))
    }

    /// Find the most recent run that should be (re-)entered from the commit
    /// page. The default commit page must prefer a freshly-frozen
    /// `ready_to_commit` run over an older `completed` run; an old completed
    /// run must not抢占 a newer ready-to-commit run just because it has a
    /// populated `completed_at`.
    ///
    /// Priority (highest first):
    /// 1. `ready_to_commit` — a frozen plan exists, nothing committed yet.
    /// 2. `cancelled` with no recoverable or unresolved terminal transaction —
    ///    cancel-before-prewrite leaves the run re-committable from the same
    ///    frozen plan.
    /// 3. `committing` / `recovery_required` with no recoverable transaction
    ///    and either a missing frozen-plan album transaction or no unresolved
    ///    terminal transaction — the idempotent commit loop creates missing
    ///    rows or reconciles an already-archived plan to completed.
    ///
    /// `completed`/`failed` runs never enter the default commit page. Within a
    /// priority tier, the newest run by `started_at` wins (the frozen plan is
    /// what makes the run committable, not the completion timestamp). Only
    /// runs with a frozen/consumed plan and a non-null `plan_hash` are eligible,
    /// so an aborted scan (still in `scanning`/`fingerprinting`/...) is never
    /// picked up here.
    pub async fn get_latest_committable_run(client: &Client) -> Result<Option<Uuid>, AppError> {
        let row = client
            .query_opt(
                "SELECT r.id FROM import_runs r
                 WHERE r.state IN ('ready_to_commit', 'cancelled', 'committing', 'recovery_required')
                   AND EXISTS (
                       SELECT 1 FROM import_plans p
                       WHERE p.import_run_id = r.id
                         AND p.state IN ('frozen', 'consumed')
                         AND p.plan_hash IS NOT NULL
                   )
                   AND (
                       r.state = 'ready_to_commit'
                       OR (r.state = 'cancelled'
                           AND NOT EXISTS (
                               SELECT 1 FROM file_transactions ft
                               WHERE ft.import_run_id = r.id
                                 AND ft.state IN (
                                     'planned', 'staging', 'verifying', 'verified',
                                     'publishing', 'published', 'db_committing',
                                     'library_committed', 'source_archiving', 'source_files_removing',
                                     'cleanup_required', 'conflict', 'failed', 'cancelled'
                                 )
                           )
                       )
                       OR (r.state IN ('committing', 'recovery_required')
                           AND NOT EXISTS (
                               SELECT 1 FROM file_transactions ft
                               WHERE ft.import_run_id = r.id
                                 AND ft.state IN (
                                     'planned', 'staging', 'verifying', 'verified',
                                     'publishing', 'published', 'db_committing',
                                     'library_committed', 'source_archiving', 'source_files_removing',
                                     'cleanup_required', 'conflict'
                                 )
                           )
                           AND (
                               EXISTS (
                                   SELECT 1
                                   FROM import_plans p2
                                   JOIN import_plan_albums ipa ON ipa.plan_id = p2.id
                                   WHERE p2.import_run_id = r.id
                                     AND p2.state IN ('frozen', 'consumed')
                                     AND p2.plan_hash IS NOT NULL
                                     AND NOT EXISTS (
                                         SELECT 1 FROM file_transactions ft
                                         WHERE ft.import_run_id = r.id
                                           AND ft.import_album_id = ipa.import_album_id
                                     )
                                 )
                               OR NOT EXISTS (
                                   SELECT 1 FROM file_transactions ft
                                   WHERE ft.import_run_id = r.id
                                     AND ft.state IN ('failed', 'cancelled')
                               )
                           )
                       )
                   )
                 ORDER BY CASE r.state
                            WHEN 'ready_to_commit' THEN 0
                            WHEN 'cancelled' THEN 1
                            WHEN 'committing' THEN 2
                            WHEN 'recovery_required' THEN 3
                          END,
                          r.started_at DESC
                 LIMIT 1",
                &[],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query latest committable run: {e}"))
            })?;

        Ok(row.map(|r| r.get("id")))
    }

    pub async fn get_import_run_by_id(
        client: &Client,
        id: Uuid,
    ) -> Result<Option<ImportRunRecord>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, source_root, library_root_id, state, policy_version, statistics, completed_at
                 FROM import_runs WHERE id = $1",
                &[&id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query import run: {e}")))?;

        Ok(row.map(|r| ImportRunRecord {
            id: r.get("id"),
            source_root: r.get("source_root"),
            library_root_id: r.get("library_root_id"),
            state: r.get("state"),
            policy_version: r.get("policy_version"),
            statistics: r.get("statistics"),
            completed_at: r.get("completed_at"),
        }))
    }

    pub async fn get_import_albums_with_source_for_run(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<ImportAlbumFullRow>, AppError> {
        let rows = client
            .query(
                "SELECT id, source_path, source_name, state FROM import_albums WHERE import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query import albums with source: {e}"))
            })?;

        Ok(rows
            .iter()
            .map(|r| ImportAlbumFullRow {
                id: r.get("id"),
                source_path: r.get("source_path"),
                source_name: r.get("source_name"),
                state: r.get("state"),
            })
            .collect())
    }

    /// Fetch a single import album by its id, including the persisted source
    /// path and state. Used by commit and recovery to read the authoritative
    /// source album directory instead of deriving it from plan images.
    pub async fn get_import_album_by_id(
        client: &Client,
        id: Uuid,
    ) -> Result<Option<ImportAlbumFullRow>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, source_path, source_name, state FROM import_albums WHERE id = $1",
                &[&id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query import album by id: {e}")))?;
        Ok(row.map(|r| ImportAlbumFullRow {
            id: r.get("id"),
            source_path: r.get("source_path"),
            source_name: r.get("source_name"),
            state: r.get("state"),
        }))
    }

    pub async fn get_import_images_by_ids(
        client: &Client,
        image_ids: &[Uuid],
    ) -> Result<Vec<ImportImageFullRow>, AppError> {
        if image_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = client
            .query(
                "SELECT id, source_path, relative_path, file_size, width, height, format,
                        blake3, pixel_hash, block_hash_16, double_gradient_hash_32,
                        fingerprint_version, import_album_id
                 FROM import_images WHERE id = ANY($1)",
                &[&image_ids],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query import images by ids: {e}"))
            })?;

        Ok(rows
            .iter()
            .map(|r| ImportImageFullRow {
                id: r.get("id"),
                source_path: r.get("source_path"),
                relative_path: r.get("relative_path"),
                file_size: r.get("file_size"),
                width: r.get("width"),
                height: r.get("height"),
                format: r.get("format"),
                blake3: r.get("blake3"),
                pixel_hash: r.get("pixel_hash"),
                block_hash_16: r.get("block_hash_16"),
                double_gradient_hash_32: r.get("double_gradient_hash_32"),
                fingerprint_version: r.get("fingerprint_version"),
                import_album_id: r.get("import_album_id"),
            })
            .collect())
    }

    pub async fn find_library_album_by_path(
        client: &Client,
        library_root_id: Uuid,
        relative_path: &str,
    ) -> Result<Option<LibraryAlbumRow>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, image_count, state FROM library_albums
                 WHERE library_root_id = $1 AND relative_path = $2",
                &[&library_root_id, &relative_path],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query library album by path: {e}"))
            })?;

        Ok(row.map(|r| LibraryAlbumRow {
            id: r.get("id"),
            image_count: r.get("image_count"),
            state: r.get("state"),
        }))
    }

    pub async fn find_existing_file_transaction(
        client: &Client,
        import_album_id: Uuid,
    ) -> Result<Option<FileTransactionRow>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, state FROM file_transactions WHERE import_album_id = $1",
                &[&import_album_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query file transaction: {e}")))?;

        Ok(row.map(|r| FileTransactionRow {
            id: r.get("id"),
            state: r.get("state"),
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn prewrite_file_transaction(
        client: &mut Client,
        transaction_id: Uuid,
        import_run_id: Uuid,
        import_album_id: Uuid,
        state: &TransactionState,
        staging_path: &str,
        target_path: &str,
        plan_hash: &[u8],
        source_file_mode: SourceFileMode,
        operations: &[NewFileOperation],
    ) -> Result<Vec<Uuid>, AppError> {
        let transaction = client.transaction().await.map_err(|e| {
            AppError::Internal(format!("failed to begin file transaction prewrite: {e}"))
        })?;
        let state = state.to_string();
        let operation_state = FileOpState::Planned.to_string();

        let result: Result<Vec<Uuid>, AppError> = async {
            transaction
                .execute(
                    "INSERT INTO file_transactions
                     (id, import_run_id, import_album_id, state, staging_path, target_path,
                      manifest_path, plan_hash, source_file_mode)
                     VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8)",
                    &[
                        &transaction_id,
                        &import_run_id,
                        &import_album_id,
                        &state,
                        &staging_path,
                        &target_path,
                        &plan_hash,
                        &source_file_mode.to_string(),
                    ],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to prewrite file transaction: {e}"))
                })?;

            let mut operation_ids = Vec::with_capacity(operations.len());
            for operation in operations {
                let operation_id = Uuid::new_v4();
                transaction
                    .execute(
                        "INSERT INTO file_operations
                         (id, transaction_id, source_path, staging_path, target_path,
                          expected_size, expected_blake3, state)
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                        &[
                            &operation_id,
                            &transaction_id,
                            &operation.source_path,
                            &operation.staging_path,
                            &operation.target_path,
                            &operation.expected_size,
                            &operation.expected_blake3,
                            &operation_state,
                        ],
                    )
                    .await
                    .map_err(|e| {
                        AppError::Internal(format!("failed to prewrite file operation: {e}"))
                    })?;
                operation_ids.push(operation_id);
                if source_file_mode == SourceFileMode::MoveSelectedWithoutBackup {
                    transaction
                        .execute(
                            "INSERT INTO source_file_cleanup_operations
                             (id, transaction_id, source_path, quarantine_path,
                              expected_size, expected_blake3, state)
                             VALUES ($1, $2, $3, $4, $5, $6, 'pending')",
                            &[
                                &Uuid::new_v4(),
                                &transaction_id,
                                &operation.source_path,
                                &operation.source_cleanup_quarantine_path,
                                &operation.expected_size,
                                &operation.expected_blake3,
                            ],
                        )
                        .await
                        .map_err(|e| {
                            AppError::Internal(format!(
                                "failed to prewrite source cleanup operation: {e}"
                            ))
                        })?;
                }
            }
            Ok(operation_ids)
        }
        .await;

        match result {
            Ok(operation_ids) => {
                transaction.commit().await.map_err(|e| {
                    AppError::Internal(format!("failed to commit file transaction prewrite: {e}"))
                })?;
                Ok(operation_ids)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn insert_file_transaction(
        client: &Client,
        transaction_id: Uuid,
        import_run_id: Uuid,
        import_album_id: Uuid,
        state: &TransactionState,
        staging_path: Option<&str>,
        target_path: Option<&str>,
        manifest_path: Option<&str>,
    ) -> Result<(), AppError> {
        let state_str = state.to_string();
        client
            .execute(
                "INSERT INTO file_transactions
                 (id, import_run_id, import_album_id, state, staging_path, target_path, manifest_path)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[
                    &transaction_id,
                    &import_run_id,
                    &import_album_id,
                    &state_str,
                    &staging_path,
                    &target_path,
                    &manifest_path,
                ],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to insert file transaction: {e}"))
            })?;
        Ok(())
    }

    pub async fn update_file_transaction_state(
        client: &Client,
        id: Uuid,
        state: &TransactionState,
        last_error: Option<&str>,
    ) -> Result<(), AppError> {
        let state_str = state.to_string();
        let completed_at = match *state {
            TransactionState::SourceArchived
            | TransactionState::SourceFilesRemoved
            | TransactionState::LibraryCommitted => Some(chrono::Utc::now()),
            _ => None,
        };
        client
            .execute(
                "UPDATE file_transactions SET state = $1, last_error = $2,
                 completed_at = COALESCE($3, completed_at) WHERE id = $4",
                &[&state_str, &last_error, &completed_at, &id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to update file transaction state: {e}"))
            })?;
        Ok(())
    }

    pub async fn insert_file_operation(
        client: &Client,
        transaction_id: Uuid,
        source_path: &str,
        staging_path: &str,
        target_path: &str,
        expected_size: i64,
        expected_blake3: &[u8],
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        let state = FileOpState::Planned.to_string();
        client
            .execute(
                "INSERT INTO file_operations
                 (id, transaction_id, source_path, staging_path, target_path,
                  expected_size, expected_blake3, state)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                &[
                    &id,
                    &transaction_id,
                    &source_path,
                    &staging_path,
                    &target_path,
                    &expected_size,
                    &expected_blake3,
                    &state,
                ],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert file operation: {e}")))?;
        Ok(id)
    }

    pub async fn update_file_operation_state(
        client: &Client,
        id: Uuid,
        state: &FileOpState,
        actual_blake3: Option<&[u8]>,
        last_error: Option<&str>,
    ) -> Result<(), AppError> {
        let state_str = state.to_string();
        client
            .execute(
                "UPDATE file_operations SET state = $1, actual_blake3 = COALESCE($2, actual_blake3),
                 last_error = $3, attempt_count = attempt_count + 1, updated_at = now()
                 WHERE id = $4",
                &[&state_str, &actual_blake3, &last_error, &id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to update file operation state: {e}"))
            })?;
        Ok(())
    }

    pub async fn update_library_root_path(
        client: &Client,
        id: Uuid,
        path: &str,
    ) -> Result<(), AppError> {
        client
            .execute(
                "UPDATE library_roots SET path = $1, updated_at = now() WHERE id = $2",
                &[&path, &id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to update library root path: {e}")))?;
        Ok(())
    }

    // ── Import Plan CRUD ──────────────────────────────────────────

    pub async fn insert_import_plan(
        client: &Client,
        import_run_id: Uuid,
        version: i32,
        state: &str,
        policy_version: &str,
        library_root_id: Uuid,
        plan_hash: Option<&[u8]>,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO import_plans (id, import_run_id, version, state, policy_version, library_root_id, plan_hash)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                &[&id, &import_run_id, &version, &state, &policy_version, &library_root_id, &plan_hash],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert import plan: {e}")))?;
        Ok(id)
    }

    /// Insert a plan and return its id, taking the state as a typed PlanState.
    pub async fn create_import_plan(
        client: &Client,
        import_run_id: Uuid,
        version: i32,
        policy_version: &str,
        library_root_id: Uuid,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        let state = PlanState::Draft.to_string();
        client
            .execute(
                "INSERT INTO import_plans (id, import_run_id, version, state, policy_version, library_root_id)
                 VALUES ($1, $2, $3, $4, $5, $6)",
                &[&id, &import_run_id, &version, &state, &policy_version, &library_root_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert import plan: {e}")))?;
        Ok(id)
    }

    pub async fn set_plan_hash(
        client: &Client,
        plan_id: Uuid,
        plan_hash: &[u8],
    ) -> Result<(), AppError> {
        client
            .execute(
                "UPDATE import_plans SET plan_hash = $1 WHERE id = $2",
                &[&plan_hash, &plan_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to set plan hash: {e}")))?;
        Ok(())
    }

    pub async fn update_import_plan_state(
        client: &Client,
        plan_id: Uuid,
        state: &PlanState,
    ) -> Result<(), AppError> {
        let state_str = state.to_string();
        let frozen_at = match *state {
            PlanState::Frozen => Some(chrono::Utc::now()),
            _ => None,
        };
        client
            .execute(
                "UPDATE import_plans SET state = $1, frozen_at = COALESCE($2, frozen_at) WHERE id = $3",
                &[&state_str, &frozen_at, &plan_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to update import plan state: {e}")))?;
        Ok(())
    }

    /// Invalidate every editable draft for a run after its review facts
    /// change. The caller must hold the parent import-run row lock and keep
    /// this update in the same transaction as the review decision write.
    pub async fn invalidate_draft_import_plans(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<u64, AppError> {
        client
            .execute(
                "UPDATE import_plans
                 SET state = 'invalidated'
                 WHERE import_run_id = $1 AND state = 'draft'",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to invalidate draft plan: {e}")))
    }

    pub async fn get_frozen_plan_for_run(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Option<(Uuid, Uuid, Vec<u8>)>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, library_root_id, plan_hash FROM import_plans
                 WHERE import_run_id = $1 AND state = 'frozen'
                 ORDER BY version DESC LIMIT 1",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query frozen plan: {e}")))?;
        Ok(row.map(|r| (r.get("id"), r.get("library_root_id"), r.get("plan_hash"))))
    }

    pub async fn insert_plan_album(
        client: &Client,
        plan_id: Uuid,
        import_album_id: Uuid,
        target_relative_path: &str,
        expected_image_count: i32,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO import_plan_albums (id, plan_id, import_album_id, target_relative_path, expected_image_count)
                 VALUES ($1, $2, $3, $4, $5)",
                &[&id, &plan_id, &import_album_id, &target_relative_path, &expected_image_count],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert plan album: {e}")))?;
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn insert_plan_image(
        client: &Client,
        plan_album_id: Uuid,
        import_image_id: Uuid,
        source_path: &str,
        source_relative_path: &str,
        target_relative_path: &str,
        expected_file_size: i64,
        expected_blake3: &[u8],
        width: Option<i32>,
        height: Option<i32>,
        format: Option<&str>,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO import_plan_images
                 (id, plan_album_id, import_image_id, source_path, source_relative_path,
                  target_relative_path, expected_file_size, expected_blake3, width, height, format)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                &[
                    &id,
                    &plan_album_id,
                    &import_image_id,
                    &source_path,
                    &source_relative_path,
                    &target_relative_path,
                    &expected_file_size,
                    &expected_blake3,
                    &width,
                    &height,
                    &format,
                ],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert plan image: {e}")))?;
        Ok(id)
    }

    pub async fn get_plan_images(
        client: &Client,
        plan_id: Uuid,
    ) -> Result<Vec<(Uuid, String, String, i64, Vec<u8>)>, AppError> {
        let rows = client
            .query(
                "SELECT ipi.import_image_id, ipi.source_path, ipi.target_relative_path,
                        ipi.expected_file_size, ipi.expected_blake3
                 FROM import_plan_images ipi
                 JOIN import_plan_albums ipa ON ipi.plan_album_id = ipa.id
                 WHERE ipa.plan_id = $1
                 ORDER BY ipi.target_relative_path",
                &[&plan_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query plan images: {e}")))?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get("import_image_id"),
                    r.get("source_path"),
                    r.get("target_relative_path"),
                    r.get("expected_file_size"),
                    r.get("expected_blake3"),
                )
            })
            .collect())
    }

    // ── Frozen plan full load (immutable commit source of truth) ──────

    /// Persist the editable plan projection without a hash. The caller owns
    /// the import-run row lock and has already validated that review is
    /// complete. Repeated generation reuses the same draft.
    pub async fn write_draft_import_plan_in_transaction(
        client: &Client,
        import_run_id: Uuid,
        albums: &[AlbumRow],
        kept_images_by_album: &HashMap<Uuid, Vec<ImportImageFullRow>>,
        policy_version: &str,
        library_root_id: Uuid,
    ) -> Result<Uuid, AppError> {
        if let Some(existing) = Self::load_draft_plan(client, import_run_id).await? {
            return Ok(existing.plan_id);
        }

        let version = Self::next_import_plan_version(client, import_run_id).await?;
        let plan_id = Self::create_import_plan(
            client,
            import_run_id,
            version,
            policy_version,
            library_root_id,
        )
        .await?;
        Self::populate_import_plan(client, plan_id, albums, kept_images_by_album).await?;
        Ok(plan_id)
    }

    /// Write every frozen-plan row inside the caller's open transaction.
    /// The caller must already hold the parent `import_runs` row lock; this
    /// keeps candidate/image reads, hash computation, writes, and summary
    /// projection behind one serialization point shared with plan edits and
    /// Commit.
    ///
    /// `albums` is the full album set for the run (each carrying its source
    /// name) and `kept_images_by_album` is, per album id, the images the
    /// plan keeps (already resolved to full rows with BLAKE3 etc.). Albums
    /// with zero kept images contribute no `import_plan_albums` row.
    pub async fn write_frozen_import_plan_in_transaction(
        client: &Client,
        import_run_id: Uuid,
        albums: &[AlbumRow],
        kept_images_by_album: &HashMap<Uuid, Vec<ImportImageFullRow>>,
        policy_version: &str,
        library_root_id: Uuid,
        plan_hash: &[u8],
    ) -> Result<Uuid, AppError> {
        // Idempotent for a second caller that waited on the run row lock.
        if let Some(existing) = Self::load_plan_in_state(client, import_run_id, "frozen").await? {
            return Ok(existing.plan_id);
        }

        let version = Self::next_import_plan_version(client, import_run_id).await?;
        let plan_id = Self::create_import_plan(
            client,
            import_run_id,
            version,
            policy_version,
            library_root_id,
        )
        .await?;

        Self::populate_import_plan(client, plan_id, albums, kept_images_by_album).await?;

        Self::set_plan_hash(client, plan_id, plan_hash).await?;
        Self::update_import_plan_state(client, plan_id, &PlanState::Frozen).await?;

        // The service validated the pre-freeze state while holding the same
        // row lock. A no-review run may already be ready_to_commit.
        let run = Self::get_import_run_by_id(client, import_run_id)
            .await?
            .ok_or_else(|| AppError::Internal(format!("import run {import_run_id} not found")))?;
        if run.state != ImportRunState::ReadyToCommit.to_string() {
            Self::update_import_run_state(client, import_run_id, &ImportRunState::ReadyToCommit)
                .await?;
        }

        Ok(plan_id)
    }

    async fn populate_import_plan(
        client: &Client,
        plan_id: Uuid,
        albums: &[AlbumRow],
        kept_images_by_album: &HashMap<Uuid, Vec<ImportImageFullRow>>,
    ) -> Result<(), AppError> {
        for album in albums {
            let Some(images) = kept_images_by_album.get(&album.id) else {
                continue;
            };
            if images.is_empty() {
                continue;
            }
            let plan_album_id = Self::insert_plan_album(
                client,
                plan_id,
                album.id,
                &album.source_name,
                images.len() as i32,
            )
            .await?;

            for img in images {
                let expected_blake3 = img.blake3.as_deref().ok_or_else(|| {
                    AppError::Internal(format!(
                        "cannot freeze plan: image {} has no BLAKE3 fingerprint",
                        img.id
                    ))
                })?;
                let target_relative_path =
                    target_relative_path_for_album_name(&album.source_name, &img.relative_path)?;
                Self::insert_plan_image(
                    client,
                    plan_album_id,
                    img.id,
                    &img.source_path,
                    &img.relative_path,
                    &target_relative_path,
                    img.file_size,
                    expected_blake3,
                    img.width,
                    img.height,
                    img.format.as_deref(),
                )
                .await?;
            }
        }

        Ok(())
    }

    /// Next 1-based version number for a new import plan on this run.
    async fn next_import_plan_version(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<i32, AppError> {
        let row = client
            .query_one(
                "SELECT COALESCE(MAX(version), 0) + 1 AS next_version
                 FROM import_plans WHERE import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query next plan version: {e}")))?;
        Ok(row.get("next_version"))
    }

    /// Read the frozen plan for a run and project it back into the public
    /// `ImportPlan` shape the commit page renders. This is the single
    /// source of truth for the commit-confirm view — it must NOT be derived
    /// by re-running `build_import_plan` (which would reflect post-freeze
    /// candidate/review edits the frozen plan intentionally ignores).
    ///
    /// `total_images` and `skipped_albums` come from the run's import
    /// albums/images (frozen at scan time, immutable for the run), not from
    /// the frozen plan rows themselves — the plan only persists what is
    /// kept, so `excluded_count` is `total_images - kept_images.len()`.
    pub async fn load_frozen_plan_summary(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Option<ImportPlan>, AppError> {
        Self::load_plan_summary(client, import_run_id, false).await
    }

    /// Read the latest editable draft projection. Drafts deliberately have
    /// no plan hash; the hash is created only by the atomic freeze step.
    pub async fn load_draft_plan_summary(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Option<ImportPlan>, AppError> {
        Self::load_plan_summary(client, import_run_id, true).await
    }

    async fn load_plan_summary(
        client: &Client,
        import_run_id: Uuid,
        draft: bool,
    ) -> Result<Option<ImportPlan>, AppError> {
        let plan = if draft {
            Self::load_draft_plan(client, import_run_id).await?
        } else {
            Self::load_frozen_plan(client, import_run_id).await?
        };
        let Some(frozen) = plan else {
            return Ok(None);
        };

        let all_images = Self::get_all_import_images_with_album(client, import_run_id).await?;
        let image_source_album: HashMap<Uuid, (Uuid, String)> = all_images
            .iter()
            .map(|img| (img.id, (img.album_id, img.album_name.clone())))
            .collect();
        let source_albums = Self::get_albums_for_run(client, import_run_id).await?;

        let mut kept_images: Vec<ImportPlanImage> = Vec::new();
        let mut included_image_ids: HashSet<Uuid> = HashSet::new();
        let mut albums: Vec<ImportPlanAlbum> = Vec::new();
        for (album, images) in &frozen.albums {
            let mut album_images = Vec::new();
            for img in images {
                included_image_ids.insert(img.import_image_id);
                let source_album_id = image_source_album
                    .get(&img.import_image_id)
                    .map(|(album_id, _)| *album_id)
                    .unwrap_or(album.import_album_id);
                let plan_image = ImportPlanImage {
                    image_id: img.import_image_id.to_string(),
                    source_path: img.source_path.clone(),
                    relative_path: img.target_relative_path.clone(),
                    file_size: img.expected_file_size,
                    album_name: album.target_relative_path.clone(),
                    album_id: album.import_album_id.to_string(),
                    source_album_id: source_album_id.to_string(),
                    included: true,
                    target_album_id: album.import_album_id.to_string(),
                    target_album_name: album.target_relative_path.clone(),
                    target_relative_path: img.target_relative_path.clone(),
                };
                kept_images.push(plan_image.clone());
                album_images.push(plan_image);
            }
            album_images.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
            albums.push(ImportPlanAlbum {
                album_id: album.import_album_id.to_string(),
                album_name: album.target_relative_path.clone(),
                included: !album_images.is_empty(),
                image_count: album_images.len() as u32,
                total_size: album_images.iter().map(|img| img.file_size).sum(),
                images: album_images,
            });
        }

        for source_album in &source_albums {
            let skipped_images: Vec<ImportPlanImage> = all_images
                .iter()
                .filter(|img| img.album_id == source_album.id)
                .filter(|img| !included_image_ids.contains(&img.id))
                .map(|img| ImportPlanImage {
                    image_id: img.id.to_string(),
                    source_path: img.source_path.clone(),
                    relative_path: img.relative_path.clone(),
                    file_size: img.file_size,
                    album_name: source_album.source_name.clone(),
                    album_id: source_album.id.to_string(),
                    source_album_id: img.album_id.to_string(),
                    included: false,
                    target_album_id: source_album.id.to_string(),
                    target_album_name: source_album.source_name.clone(),
                    target_relative_path: img.relative_path.clone(),
                })
                .collect();

            if skipped_images.is_empty() {
                if !albums
                    .iter()
                    .any(|album| album.album_id == source_album.id.to_string())
                {
                    albums.push(ImportPlanAlbum {
                        album_id: source_album.id.to_string(),
                        album_name: source_album.source_name.clone(),
                        included: false,
                        image_count: 0,
                        total_size: 0,
                        images: Vec::new(),
                    });
                }
                continue;
            }

            if let Some(existing) = albums
                .iter_mut()
                .find(|album| album.album_id == source_album.id.to_string())
            {
                existing.images.extend(skipped_images);
                existing
                    .images
                    .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
            } else {
                albums.push(ImportPlanAlbum {
                    album_id: source_album.id.to_string(),
                    album_name: source_album.source_name.clone(),
                    included: false,
                    image_count: 0,
                    total_size: 0,
                    images: skipped_images,
                });
            }
        }

        for album in &mut albums {
            album.image_count = album.images.iter().filter(|img| img.included).count() as u32;
            album.total_size = album
                .images
                .iter()
                .filter(|img| img.included)
                .map(|img| img.file_size)
                .sum();
            album.included = album.image_count > 0;
        }
        albums.sort_by(|a, b| a.album_name.cmp(&b.album_name));

        // Stable order by target path so the commit-confirm view does not
        // flicker between reads.
        kept_images.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        let total_images = all_images.len() as i64;

        let total_albums: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM import_albums WHERE import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to count import albums: {e}")))?
            .get(0);

        // Skipped albums: import albums with no plan album in the frozen plan.
        let plan_album_ids: Vec<Uuid> = frozen
            .albums
            .iter()
            .map(|(a, _)| a.import_album_id)
            .collect();
        let skipped_rows = if plan_album_ids.is_empty() {
            client
                .query(
                    "SELECT source_name FROM import_albums WHERE import_run_id = $1
                     ORDER BY source_name",
                    &[&import_run_id],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to query skipped albums: {e}")))?
        } else {
            client
                .query(
                    "SELECT source_name FROM import_albums WHERE import_run_id = $1
                     AND id <> ALL($2)
                     ORDER BY source_name",
                    &[&import_run_id, &plan_album_ids],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to query skipped albums: {e}")))?
        };
        let skipped_albums: Vec<String> = skipped_rows
            .iter()
            .map(|r| r.get::<_, String>("source_name"))
            .collect();

        let kept_count = kept_images.len() as u64;
        Ok(Some(ImportPlan {
            import_run_id: import_run_id.to_string(),
            plan_hash: frozen.plan_hash.as_ref().map(|hash| {
                hash.iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            }),
            source_file_mode: frozen.source_file_mode,
            total_albums: total_albums as u32,
            total_images: total_images as u32,
            kept_images,
            excluded_count: (total_images as u64).saturating_sub(kept_count) as u32,
            skipped_albums,
            albums,
        }))
    }

    /// Load the active committable plan for a run: the frozen plan if one
    /// exists, otherwise the most recently consumed plan (so idempotent
    /// reruns can verify already-committed albums). Returns None if the run
    /// has neither a frozen nor a consumed plan.
    pub async fn load_frozen_plan(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Option<FrozenPlanRow>, AppError> {
        // Prefer a frozen plan; fall back to the latest consumed one for
        // idempotent recovery of an already-completed commit.
        if let Some(p) = Self::load_plan_in_state(client, import_run_id, "frozen").await? {
            return Ok(Some(p));
        }
        Self::load_plan_in_state(client, import_run_id, "consumed").await
    }

    /// Load the latest draft plan for a run (used to compute its hash before
    /// freezing). Returns None if there is no draft plan.
    pub async fn load_draft_plan(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Option<FrozenPlanRow>, AppError> {
        Self::load_plan_in_state(client, import_run_id, "draft").await
    }

    async fn load_plan_in_state(
        client: &Client,
        import_run_id: Uuid,
        state: &str,
    ) -> Result<Option<FrozenPlanRow>, AppError> {
        let header = client
            .query_opt(
                "SELECT id, import_run_id, library_root_id, state, plan_hash, policy_version,
                        source_file_mode
                 FROM import_plans
                 WHERE import_run_id = $1 AND state = $2
                 ORDER BY version DESC LIMIT 1",
                &[&import_run_id, &state],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to load {state} plan: {e}")))?;
        let Some(header) = header else {
            return Ok(None);
        };
        let plan_id: Uuid = header.get("id");
        let plan_hash: Option<Vec<u8>> = header.get("plan_hash");

        let album_rows = client
            .query(
                "SELECT id, import_album_id, target_relative_path, expected_image_count, album_plan_hash
                 FROM import_plan_albums WHERE plan_id = $1
                 ORDER BY target_relative_path",
                &[&plan_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to load plan albums: {e}")))?;

        let mut albums: Vec<(PlanAlbumRow, Vec<PlanImageRow>)> = Vec::new();
        for ar in &album_rows {
            let plan_album_id: Uuid = ar.get("id");
            let images = client
                .query(
                    "SELECT id, plan_album_id, import_image_id, source_path, source_relative_path,
                            target_relative_path, expected_file_size, expected_blake3,
                            width, height, format
                     FROM import_plan_images WHERE plan_album_id = $1
                     ORDER BY target_relative_path",
                    &[&plan_album_id],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to load plan images: {e}")))?;
            let album = PlanAlbumRow {
                plan_album_id,
                import_album_id: ar.get("import_album_id"),
                target_relative_path: ar.get("target_relative_path"),
                expected_image_count: ar.get("expected_image_count"),
                album_plan_hash: ar.get("album_plan_hash"),
            };
            let imgs: Vec<PlanImageRow> = images
                .iter()
                .map(|r| PlanImageRow {
                    id: r.get("id"),
                    plan_album_id: r.get("plan_album_id"),
                    import_image_id: r.get("import_image_id"),
                    source_path: r.get("source_path"),
                    source_relative_path: r.get("source_relative_path"),
                    target_relative_path: r.get("target_relative_path"),
                    expected_file_size: r.get("expected_file_size"),
                    expected_blake3: r.get("expected_blake3"),
                    width: r.get("width"),
                    height: r.get("height"),
                    format: r.get("format"),
                })
                .collect();
            albums.push((album, imgs));
        }

        Ok(Some(FrozenPlanRow {
            plan_id,
            import_run_id: header.get("import_run_id"),
            library_root_id: header.get("library_root_id"),
            plan_state: header.get("state"),
            plan_hash,
            policy_version: header.get("policy_version"),
            source_file_mode: SourceFileMode::from_str_opt(
                &header.get::<_, String>("source_file_mode"),
            )
            .ok_or_else(|| {
                AppError::Internal("invalid import plan source_file_mode".to_string())
            })?,
            albums,
        }))
    }

    // ── File transaction recovery queries ──────────────────────────────

    pub async fn get_file_transaction(
        client: &Client,
        transaction_id: Uuid,
    ) -> Result<Option<FileTransactionFullRow>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, import_run_id, import_album_id, state, staging_path, target_path,
                        manifest_path, plan_hash, manifest_hash, source_file_mode, last_error
                 FROM file_transactions WHERE id = $1",
                &[&transaction_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query file transaction: {e}")))?;
        Ok(row.map(|r| FileTransactionFullRow {
            id: r.get("id"),
            import_run_id: r.get("import_run_id"),
            import_album_id: r.get("import_album_id"),
            state: r.get("state"),
            staging_path: r.get("staging_path"),
            target_path: r.get("target_path"),
            manifest_path: r.get("manifest_path"),
            plan_hash: r.get("plan_hash"),
            manifest_hash: r.get("manifest_hash"),
            source_file_mode: SourceFileMode::from_str_opt(&r.get::<_, String>("source_file_mode"))
                .expect("source_file_mode database constraint"),
            last_error: r.get("last_error"),
        }))
    }

    /// All actionable transaction diagnostics at startup. This includes
    /// failed/cancelled terminal rows so the Recovery surface can present an
    /// explicit manual-disposition state instead of an empty page. Only
    /// source_archived is omitted because it is fully resolved.
    pub async fn get_recoverable_transactions(
        client: &Client,
    ) -> Result<Vec<FileTransactionFullRow>, AppError> {
        let rows = client
            .query(
                "SELECT ft.id, ft.import_run_id, ft.import_album_id, ft.state,
                        ft.staging_path, ft.target_path, ft.manifest_path,
                        ft.plan_hash, ft.manifest_hash, ft.source_file_mode, ft.last_error
                 FROM file_transactions ft
                 JOIN import_runs r ON r.id = ft.import_run_id
                 WHERE ft.state NOT IN ('source_archived', 'source_files_removed')
                   AND r.state NOT IN ('abandoned', 'completed')
                 ORDER BY ft.started_at",
                &[],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query recoverable transactions: {e}"))
            })?;
        Ok(rows
            .iter()
            .map(|r| FileTransactionFullRow {
                id: r.get("id"),
                import_run_id: r.get("import_run_id"),
                import_album_id: r.get("import_album_id"),
                state: r.get("state"),
                staging_path: r.get("staging_path"),
                target_path: r.get("target_path"),
                manifest_path: r.get("manifest_path"),
                plan_hash: r.get("plan_hash"),
                manifest_hash: r.get("manifest_hash"),
                source_file_mode: SourceFileMode::from_str_opt(
                    &r.get::<_, String>("source_file_mode"),
                )
                .expect("source_file_mode database constraint"),
                last_error: r.get("last_error"),
            })
            .collect())
    }

    /// All file transactions for a given import run, projected to the
    /// columns required by run-level reconciliation.
    pub async fn get_all_transactions_for_run(
        client: &Client,
        import_run_id: Uuid,
    ) -> Result<Vec<FileTransactionStateRow>, AppError> {
        let rows = client
            .query(
                "SELECT id, import_run_id, import_album_id, state
                 FROM file_transactions
                 WHERE import_run_id = $1
                 ORDER BY started_at",
                &[&import_run_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!(
                    "failed to query transactions for run {import_run_id}: {e}"
                ))
            })?;
        Ok(rows
            .iter()
            .map(|r| FileTransactionStateRow {
                id: r.get("id"),
                import_run_id: r.get("import_run_id"),
                import_album_id: r.get("import_album_id"),
                state: r.get("state"),
            })
            .collect())
    }

    /// A persisted file operation with all prewritten evidence.
    pub async fn get_file_operations(
        client: &Client,
        transaction_id: Uuid,
    ) -> Result<Vec<FileOperationRow>, AppError> {
        let rows = client
            .query(
                "SELECT id, transaction_id, source_path, staging_path, target_path,
                        expected_size, expected_blake3, actual_blake3, state, last_error
                 FROM file_operations WHERE transaction_id = $1
                 ORDER BY target_path",
                &[&transaction_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query file operations: {e}")))?;
        Ok(rows
            .iter()
            .map(|r| FileOperationRow {
                id: r.get("id"),
                transaction_id: r.get("transaction_id"),
                source_path: r.get("source_path"),
                staging_path: r.get("staging_path"),
                target_path: r.get("target_path"),
                expected_size: r.get("expected_size"),
                expected_blake3: r.get("expected_blake3"),
                actual_blake3: r.get("actual_blake3"),
                state: r.get("state"),
                last_error: r.get("last_error"),
            })
            .collect())
    }

    pub async fn get_source_file_cleanup_operations(
        client: &Client,
        transaction_id: Uuid,
    ) -> Result<Vec<SourceFileCleanupOperationRow>, AppError> {
        let rows = client
            .query(
                "SELECT id, transaction_id, source_path, quarantine_path,
                        expected_size, expected_blake3,
                        state, last_error
                 FROM source_file_cleanup_operations
                 WHERE transaction_id = $1
                 ORDER BY source_path",
                &[&transaction_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query source cleanup operations: {e}"))
            })?;
        Ok(rows
            .iter()
            .map(|row| SourceFileCleanupOperationRow {
                id: row.get("id"),
                transaction_id: row.get("transaction_id"),
                source_path: row.get("source_path"),
                quarantine_path: row.get("quarantine_path"),
                expected_size: row.get("expected_size"),
                expected_blake3: row.get("expected_blake3"),
                state: row.get("state"),
                last_error: row.get("last_error"),
            })
            .collect())
    }

    pub async fn update_source_file_cleanup_operation(
        client: &Client,
        operation_id: Uuid,
        state: &str,
        last_error: Option<&str>,
    ) -> Result<(), AppError> {
        client
            .execute(
                "UPDATE source_file_cleanup_operations
                 SET state = $1, last_error = $2, updated_at = now()
                 WHERE id = $3",
                &[&state, &last_error, &operation_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to update source cleanup operation: {e}"))
            })?;
        Ok(())
    }

    pub async fn initialize_source_cleanup_quarantine_if_missing(
        client: &Client,
        operation_id: Uuid,
        quarantine_path: &str,
        legacy_normalized_state: &str,
    ) -> Result<(String, String), AppError> {
        let row = client
            .query_one(
                "UPDATE source_file_cleanup_operations
                 SET state = CASE
                         WHEN quarantine_path IS NULL THEN $2
                         ELSE state
                     END,
                     quarantine_path = COALESCE(quarantine_path, $1),
                     last_error = CASE
                         WHEN quarantine_path IS NULL THEN NULL
                         ELSE last_error
                     END,
                     updated_at = now()
                 WHERE id = $3
                 RETURNING quarantine_path, state",
                &[&quarantine_path, &legacy_normalized_state, &operation_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!(
                    "failed to persist source cleanup quarantine path: {e}"
                ))
            })?;
        Ok((row.get("quarantine_path"), row.get("state")))
    }

    /// Persist the plan hash and manifest hash on the transaction.
    pub async fn set_transaction_hashes(
        client: &Client,
        transaction_id: Uuid,
        plan_hash: Option<&[u8]>,
        manifest_hash: Option<&[u8]>,
    ) -> Result<(), AppError> {
        client
            .execute(
                "UPDATE file_transactions
                 SET plan_hash = COALESCE($1, plan_hash),
                     manifest_hash = COALESCE($2, manifest_hash)
                 WHERE id = $3",
                &[&plan_hash, &manifest_hash, &transaction_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to set transaction hashes: {e}")))?;
        Ok(())
    }

    /// Persist the manifest path on the transaction.
    pub async fn set_transaction_manifest_path(
        client: &Client,
        transaction_id: Uuid,
        manifest_path: &str,
    ) -> Result<(), AppError> {
        client
            .execute(
                "UPDATE file_transactions SET manifest_path = $1 WHERE id = $2",
                &[&manifest_path, &transaction_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to set manifest path: {e}")))?;
        Ok(())
    }

    /// Fetch the latest file transaction for an album regardless of state
    /// (used to detect idempotent already-committed albums).
    pub async fn find_latest_file_transaction(
        client: &Client,
        import_album_id: Uuid,
    ) -> Result<Option<FileTransactionFullRow>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, import_run_id, import_album_id, state, staging_path, target_path,
                        manifest_path, plan_hash, manifest_hash, source_file_mode, last_error
                 FROM file_transactions
                 WHERE import_album_id = $1
                 ORDER BY started_at DESC LIMIT 1",
                &[&import_album_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query latest file transaction: {e}"))
            })?;
        Ok(row.map(|r| FileTransactionFullRow {
            id: r.get("id"),
            import_run_id: r.get("import_run_id"),
            import_album_id: r.get("import_album_id"),
            state: r.get("state"),
            staging_path: r.get("staging_path"),
            target_path: r.get("target_path"),
            manifest_path: r.get("manifest_path"),
            plan_hash: r.get("plan_hash"),
            manifest_hash: r.get("manifest_hash"),
            source_file_mode: SourceFileMode::from_str_opt(&r.get::<_, String>("source_file_mode"))
                .expect("source_file_mode database constraint"),
            last_error: r.get("last_error"),
        }))
    }

    // ── Library root identity ───────────────────────────────────────────

    pub async fn get_library_root_path(
        client: &Client,
        library_root_id: Uuid,
    ) -> Result<String, AppError> {
        let row = client
            .query_opt(
                "SELECT path FROM library_roots WHERE id = $1",
                &[&library_root_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query library root path: {e}")))?;
        row.map(|r| r.get::<_, String>("path"))
            .ok_or_else(|| AppError::Internal(format!("library_root {library_root_id} not found")))
    }

    pub async fn find_library_root_by_path(
        client: &Client,
        path: &str,
    ) -> Result<Option<Uuid>, AppError> {
        let row = client
            .query_opt(
                "SELECT id FROM library_roots WHERE path = $1 ORDER BY created_at LIMIT 1",
                &[&path],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query library root by path: {e}"))
            })?;
        Ok(row.map(|r| r.get("id")))
    }

    pub async fn create_library_root(
        client: &Client,
        path: &str,
        display_name: &str,
    ) -> Result<Uuid, AppError> {
        let id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO library_roots (id, path, display_name, is_active)
                 VALUES ($1, $2, $3, TRUE)",
                &[&id, &path, &display_name],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to insert library root: {e}")))?;
        Ok(id)
    }

    // ── Library commit records ─────────────────────────────────────────

    pub async fn get_library_album(
        client: &Client,
        library_root_id: Uuid,
        relative_path: &str,
    ) -> Result<Option<LibraryAlbumFullRow>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, library_root_id, display_name, relative_path, manifest_version,
                        manifest_hash, image_count, state, plan_hash, transaction_id
                 FROM library_albums
                 WHERE library_root_id = $1 AND relative_path = $2",
                &[&library_root_id, &relative_path],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query library album: {e}")))?;
        Ok(row.map(|r| LibraryAlbumFullRow {
            id: r.get("id"),
            library_root_id: r.get("library_root_id"),
            display_name: r.get("display_name"),
            relative_path: r.get("relative_path"),
            manifest_version: r.get("manifest_version"),
            manifest_hash: r.get("manifest_hash"),
            image_count: r.get("image_count"),
            state: r.get("state"),
            plan_hash: r.get("plan_hash"),
            transaction_id: r.get("transaction_id"),
        }))
    }

    pub async fn get_library_images_for_album(
        client: &Client,
        album_id: Uuid,
    ) -> Result<Vec<LibraryImageFullRow>, AppError> {
        let rows = client
            .query(
                "SELECT id, relative_path, file_size, blake3, state
                 FROM library_images WHERE album_id = $1 ORDER BY relative_path",
                &[&album_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query library images: {e}")))?;
        Ok(rows
            .iter()
            .map(|r| LibraryImageFullRow {
                id: r.get("id"),
                relative_path: r.get("relative_path"),
                file_size: r.get("file_size"),
                blake3: r.get("blake3"),
                state: r.get("state"),
            })
            .collect())
    }

    pub async fn get_library_catalog_totals(client: &Client) -> Result<(i64, i64, i64), AppError> {
        let row = client
            .query_one(
                "SELECT
                    (SELECT COUNT(*) FROM library_albums WHERE state = 'committed')::BIGINT AS total_albums,
                    (SELECT COUNT(*)
                       FROM library_images li
                       JOIN library_albums la ON la.id = li.album_id
                      WHERE li.state = 'committed' AND la.state = 'committed')::BIGINT AS total_images,
                    (SELECT COALESCE(SUM(li.file_size), 0)
                       FROM library_images li
                       JOIN library_albums la ON la.id = li.album_id
                      WHERE li.state = 'committed' AND la.state = 'committed')::BIGINT AS total_size",
                &[],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query library catalog totals: {e}"))
            })?;
        Ok((
            row.get("total_albums"),
            row.get("total_images"),
            row.get("total_size"),
        ))
    }

    pub async fn list_library_albums_page(
        client: &Client,
        cursor: Option<&LibraryAlbumKeyset>,
        limit: i64,
    ) -> Result<Vec<LibraryAlbumDetailRow>, AppError> {
        let cursor_committed_at = cursor.map(|value| value.committed_at);
        let cursor_display_name = cursor.map(|value| value.display_name.clone());
        let cursor_id = cursor.map(|value| value.id);
        let rows = client
            .query(
                "SELECT la.id, la.library_root_id, lr.path AS library_root_path,
                        la.display_name, la.relative_path,
                        COUNT(li.id)::INT AS image_count, la.state,
                        la.committed_at, COALESCE(SUM(li.file_size), 0)::BIGINT AS total_size
                 FROM library_albums la
                 JOIN library_roots lr ON lr.id = la.library_root_id
                 LEFT JOIN library_images li ON li.album_id = la.id AND li.state = 'committed'
                 WHERE la.state = 'committed'
                   AND (
                       $1::TIMESTAMPTZ IS NULL
                       OR la.committed_at < $1
                       OR (la.committed_at = $1 AND la.display_name > $2)
                       OR (la.committed_at = $1 AND la.display_name = $2 AND la.id > $3)
                   )
                 GROUP BY la.id, la.library_root_id, lr.path, la.display_name,
                          la.relative_path, la.state, la.committed_at
                 ORDER BY la.committed_at DESC, la.display_name, la.id
                 LIMIT $4",
                &[
                    &cursor_committed_at,
                    &cursor_display_name,
                    &cursor_id,
                    &limit,
                ],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to list library albums: {e}")))?;
        Ok(rows
            .iter()
            .map(|row| LibraryAlbumDetailRow {
                id: row.get("id"),
                library_root_id: row.get("library_root_id"),
                library_root_path: row.get("library_root_path"),
                display_name: row.get("display_name"),
                relative_path: row.get("relative_path"),
                image_count: row.get("image_count"),
                total_size: row.get("total_size"),
                state: row.get("state"),
                committed_at: row.get("committed_at"),
            })
            .collect())
    }

    pub async fn get_library_album_image_totals(
        client: &Client,
        album_id: Uuid,
    ) -> Result<Option<(i64, i64)>, AppError> {
        let row = client
            .query_opt(
                "SELECT COUNT(li.id)::BIGINT AS total_images,
                        COALESCE(SUM(li.file_size), 0)::BIGINT AS total_size
                 FROM library_albums la
                 LEFT JOIN library_images li ON li.album_id = la.id AND li.state = 'committed'
                 WHERE la.id = $1 AND la.state = 'committed'
                 GROUP BY la.id",
                &[&album_id],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to query library album totals: {e}"))
            })?;
        Ok(row.map(|row| (row.get("total_images"), row.get("total_size"))))
    }

    pub async fn list_library_images_page(
        client: &Client,
        album_id: Uuid,
        cursor: Option<&LibraryImageKeyset>,
        limit: i64,
    ) -> Result<Vec<LibraryImageDetailRow>, AppError> {
        let cursor_relative_path = cursor.map(|value| value.relative_path.clone());
        let cursor_id = cursor.map(|value| value.id);
        let rows = client
            .query(
                "SELECT id, relative_path, file_size, width, height, format, state
                 FROM library_images
                 WHERE album_id = $1 AND state = 'committed'
                   AND (
                       $2::TEXT IS NULL
                       OR relative_path > $2
                       OR (relative_path = $2 AND id > $3)
                   )
                 ORDER BY relative_path, id
                 LIMIT $4",
                &[&album_id, &cursor_relative_path, &cursor_id, &limit],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to list library images: {e}")))?;
        Ok(rows
            .iter()
            .map(|row| LibraryImageDetailRow {
                id: row.get("id"),
                relative_path: row.get("relative_path"),
                file_size: row.get("file_size"),
                width: row.get("width"),
                height: row.get("height"),
                format: row.get("format"),
                state: row.get("state"),
            })
            .collect())
    }

    pub async fn insert_source_album_snapshot(
        client: &Client,
        snapshot_id: Uuid,
        import_run_id: Uuid,
        import_album_id: Uuid,
        source_album_path: &str,
        snapshot_hash: &[u8],
        files: &[NewSnapshotFile],
    ) -> Result<(), AppError> {
        client.batch_execute("BEGIN").await.map_err(|e| {
            AppError::Internal(format!("failed to begin source snapshot transaction: {e}"))
        })?;

        let result = async {
            client
                .execute(
                    "INSERT INTO source_album_snapshots
                        (id, import_run_id, import_album_id, source_album_path, snapshot_hash)
                     VALUES ($1, $2, $3, $4, $5)",
                    &[
                        &snapshot_id,
                        &import_run_id,
                        &import_album_id,
                        &source_album_path,
                        &snapshot_hash,
                    ],
                )
                .await
                .map_err(|e| {
                    AppError::Internal(format!("failed to insert source_album_snapshot: {e}"))
                })?;

            for f in files {
                client
                    .execute(
                        "INSERT INTO source_album_snapshot_files
                            (id, snapshot_id, relative_path, file_type, file_size, blake3)
                         VALUES ($1, $2, $3, $4, $5, $6)",
                        &[
                            &Uuid::new_v4(),
                            &snapshot_id,
                            &f.relative_path,
                            &f.file_type,
                            &f.file_size,
                            &f.blake3,
                        ],
                    )
                    .await
                    .map_err(|e| {
                        AppError::Internal(format!(
                            "failed to insert snapshot file '{}': {e}",
                            f.relative_path
                        ))
                    })?;
            }

            // NOTE: the snapshot hash is the single source of truth on
            // source_album_snapshots.snapshot_hash. We deliberately do NOT
            // mirror it onto import_albums.source_snapshot_hash anymore —
            // migration 0009 dropped that column because the commit /
            // recovery main chain never cross-checked it, so it was
            // redundant-evidence rather than a real guard.

            Ok(())
        }
        .await;

        if let Err(e) = result {
            let _ = client.batch_execute("ROLLBACK").await;
            return Err(e);
        }

        client.batch_execute("COMMIT").await.map_err(|e| {
            AppError::Internal(format!("failed to commit source snapshot transaction: {e}"))
        })?;

        Ok(())
    }

    pub async fn get_source_album_snapshot(
        client: &Client,
        import_album_id: Uuid,
    ) -> Result<Option<SourceAlbumSnapshotRecord>, AppError> {
        let row = client
            .query_opt(
                "SELECT id, import_run_id, import_album_id, source_album_path, snapshot_hash, created_at
                 FROM source_album_snapshots WHERE import_album_id = $1",
                &[&import_album_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query snapshot: {e}")))?;
        Ok(row.map(|r| SourceAlbumSnapshotRecord {
            snapshot_id: r.get("id"),
            import_run_id: r.get("import_run_id"),
            import_album_id: r.get("import_album_id"),
            source_album_path: r.get("source_album_path"),
            snapshot_hash: r.get("snapshot_hash"),
            created_at: r.get("created_at"),
        }))
    }

    pub async fn get_snapshot_files(
        client: &Client,
        snapshot_id: Uuid,
    ) -> Result<Vec<SnapshotFileRecord>, AppError> {
        let rows = client
            .query(
                "SELECT id, snapshot_id, relative_path, file_type, file_size, blake3
                 FROM source_album_snapshot_files
                 WHERE snapshot_id = $1 ORDER BY relative_path",
                &[&snapshot_id],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query snapshot files: {e}")))?;
        Ok(rows
            .iter()
            .map(|r| SnapshotFileRecord {
                id: r.get("id"),
                snapshot_id: r.get("snapshot_id"),
                relative_path: r.get("relative_path"),
                file_type: r.get("file_type"),
                file_size: r.get("file_size"),
                blake3: r.get("blake3"),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_scan_state_is_derived_from_persisted_facts() {
        assert_eq!(
            scan_run_state_from_facts(1, 0, 0),
            ImportRunState::Cancelled
        );
        assert_eq!(scan_run_state_from_facts(0, 1, 0), ImportRunState::Failed);
        assert_eq!(
            scan_run_state_from_facts(0, 0, 1),
            ImportRunState::ReviewRequired
        );
        assert_eq!(
            scan_run_state_from_facts(0, 0, 0),
            ImportRunState::ReadyToCommit,
            "a cancel after the last album checkpoint must remain committable"
        );
    }
}

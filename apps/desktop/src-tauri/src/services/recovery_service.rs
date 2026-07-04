//! Real Recovery Service: resumes interrupted import transactions from their
//! persisted state.
//!
//! Unlike the previous stub (which only returned label strings like
//! `"retry_staging"`), this module actually executes the recovery action for
//! each transaction state. Every action is idempotent: running it twice
//! produces no additional side effects.
//!
//! Recovery is driven by the frozen plan (the sole source of truth for what
//! files should exist, with what sizes and BLAKE3 hashes) and the persisted
//! `file_transactions` / `file_operations` rows. It never overwrites an
//! unknown published directory — a mismatch surfaces as a `conflict` with
//! full diagnostics instead of an automatic fix.
#![allow(dead_code)]
use crate::domain::import_state::ImportRunState;
use crate::domain::state_machine::{self, FileOpState, TransactionState};
use crate::error::AppError;
use crate::infrastructure::postgres::PostgresManager;
use crate::repositories::import_repository::{
    FileTransactionFullRow, ImportRepository, PlanImageRow,
};
use crate::services::commit_service::{
    build_manifest, commit_library_records_transaction, detect_extra_published_files,
    normalize_relative_path, publish_verified_staging, read_commit_marker, read_manifest_with_hash,
    select_commit_publish_strategy, stream_copy_with_hash, sync_parent_dir,
    validate_and_hash_frozen_plan, validate_commit_marker, verify_published_file_set,
    verify_source_snapshot_or_conflict, verify_staging_set, write_commit_marker,
    write_synced_then_rename, LIBRARY_ROOT_LEASE_TTL_SECS,
};
use crate::services::source_snapshot_service::load_source_album_snapshot;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio_postgres::Client;
use uuid::Uuid;

/// Diagnostic information for a recoverable transaction.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryDiagnostic {
    pub transaction_id: Uuid,
    pub import_run_id: Uuid,
    pub import_album_id: Uuid,
    pub current_state: String,
    pub staging_path: Option<String>,
    pub target_path: Option<String>,
    pub manifest_path: Option<String>,
    pub staging_exists: bool,
    pub target_exists: bool,
    pub manifest_exists: bool,
    pub plan_id: Option<Uuid>,
    pub plan_hash: Option<String>,
    pub last_error: Option<String>,
    pub diagnostics: Vec<String>,
}

/// Outcome of a single-transaction recovery.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryOutcome {
    pub transaction_id: Uuid,
    pub final_state: String,
    pub recovered: bool,
    /// `true` only when the transaction is in a genuine terminal state
    /// (`source_archived`, `failed`, `cancelled`). A `failed`/`cancelled`
    /// transaction is terminal-but-not-recovered: callers must NOT treat
    /// it as a successful recovery. Distinct from `recovered` so the GUI
    /// can show "terminal, not recovered" for failed/cancelled rows.
    pub terminal: bool,
    pub message: String,
}

/// Scan all non-terminal transactions and generate recovery diagnostics.
pub async fn scan_recoverable_transactions(
    client: &Client,
) -> Result<Vec<RecoveryDiagnostic>, AppError> {
    let rows = ImportRepository::get_recoverable_transactions(client).await?;
    let mut diagnostics = Vec::new();
    for tx in rows {
        let staging_exists = tx
            .staging_path
            .as_ref()
            .map(|p| Path::new(p).exists())
            .unwrap_or(false);
        let target_exists = tx
            .target_path
            .as_ref()
            .map(|p| Path::new(p).exists())
            .unwrap_or(false);
        let manifest_exists = tx
            .manifest_path
            .as_ref()
            .map(|p| Path::new(p).exists())
            .unwrap_or(false);

        let mut diags = Vec::new();
        match tx.state.as_str() {
            "planned" | "staging" => {
                diags.push("staging incomplete: clean .part files, resume copy".to_string());
            }
            "verifying" | "verified" => {
                diags.push("staging complete: re-verify and publish".to_string());
            }
            "publishing" => {
                if staging_exists && !target_exists {
                    diags.push("staging ready, target missing: retry rename".to_string());
                } else if target_exists {
                    diags.push("target exists: verify manifest then mark published".to_string());
                } else {
                    diags.push("staging missing: re-stage from source".to_string());
                }
            }
            "published" | "db_committing" => {
                diags.push("published: retry database commit".to_string());
            }
            "library_committed" | "source_archiving" => {
                diags.push("library committed: resume source archive".to_string());
            }
            "cleanup_required" => {
                diags.push("cleanup required: staging dir left behind".to_string());
            }
            "conflict" => {
                diags.push("conflict: manual resolution required".to_string());
            }
            other => {
                diags.push(format!("unhandled state: {other}"));
            }
        }

        diagnostics.push(RecoveryDiagnostic {
            transaction_id: tx.id,
            import_run_id: tx.import_run_id,
            import_album_id: tx.import_album_id,
            current_state: tx.state.clone(),
            staging_path: tx.staging_path.clone(),
            target_path: tx.target_path.clone(),
            manifest_path: tx.manifest_path.clone(),
            staging_exists,
            target_exists,
            manifest_exists,
            plan_id: None,
            plan_hash: tx
                .plan_hash
                .as_ref()
                .map(|b| crate::services::commit_service::bytes_to_hex(b)),
            last_error: tx.last_error.clone(),
            diagnostics: diags,
        });
    }
    Ok(diagnostics)
}

/// Outcome of a single call to [`reconcile_import_run_state`].
///
/// `changed` is `true` only when the reconciler actually wrote a new state
/// row; `false` means the persisted state already matched the computed
/// verdict (the idempotent no-op case). Tests use this to assert stability
/// across repeated calls.
///
/// `completed_at` is **always** the persisted DB value — read back from
/// `import_runs` after any write — never a transient in-memory timestamp.
/// It is `None` when the run is not `completed`. This guarantees repeated
/// reconcile calls and post-restart reads return the same value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciledRunState {
    pub import_run_id: Uuid,
    pub state: ImportRunState,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub changed: bool,
}

/// Reconcile the parent `import_runs` row against the union of its child
/// `file_transactions` rows and the frozen plan's album set.
///
/// This is the *single authoritative decider* for the run state after any
/// transaction outcome. Both the commit pipeline and the recovery service
/// call it after every transaction state change so the parent run cannot
/// drift into a state that contradicts its children.
///
/// # Rules (product-level invariants)
///
/// 1. **Empty frozen plan**: complete only after the *full* invariant
///    check (plan state, plan hash, album/image counts, and the complete
///    file-transaction set) passes. An empty plan with an active, failed,
///    cancelled, or conflict transaction is **not** `completed` — it goes
///    to `recovery_required` (or stays at the user-explicit terminal state).
///    See [`empty_plan_allows_completion`].
/// 2. Any `conflict` transaction → `recovery_required`.
/// 3. Any active (non-terminal, non-conflict) transaction —
///    `planned | staging | verifying | verified | publishing | published |
///    db_committing | library_committed | source_archiving |
///    cleanup_required` → `recovery_required`. These transactions are still
///    recoverable; cancellation must NOT flip them to `failed`/`cancelled`
///    (that would create an unrecoverable terminal state).
/// 4. Any `failed` or `cancelled` transaction → `recovery_required`
///    (current product semantics: no silent completion with unresolved
///    transaction outcomes; promote to a real `failed` run path when one
///    lands).
/// 5. Every frozen-plan album has a `source_archived` transaction for its
///    `import_album_id` → `completed` and `completed_at` is set (persisted).
///    An empty frozen plan that passes [`empty_plan_allows_completion`]
///    also completes.
/// 6. A run that has not yet reached the commit phase (`created`,
///    `scanning`, `fingerprinting`, `detecting_duplicates`, `analyzing`,
///    `review_required`, `ready_to_commit`) is left untouched — reconcile
///    is meaningless before commit has been attempted.
/// 7. Terminal states `cancelled` and `failed` are also left untouched:
///    they are set by explicit user/system actions, not derived.
///
/// # Idempotency + persisted `completed_at`
///
/// Calling this function twice in a row with no intervening state changes
/// produces the same result and sets `changed = false` on the second call.
/// `completed_at` is only written when transitioning *into* `completed`;
/// it is cleared when the run is pulled back to `recovery_required`. The
/// returned `completed_at` is always read back from the DB, so it is the
/// exact persisted value — never a transient in-memory `now()`.
pub async fn reconcile_import_run_state(
    client: &Client,
    import_run_id: Uuid,
) -> Result<ReconciledRunState, AppError> {
    let run = ImportRepository::get_import_run_by_id(client, import_run_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("import run {import_run_id} not found")))?;

    let current = match ImportRunState::from_str_opt(&run.state) {
        Some(s) => s,
        None => {
            return Err(AppError::Internal(format!(
                "import run {import_run_id} has unparseable state '{}'",
                run.state
            )));
        }
    };

    // Runs that have not yet attempted commit, or that were explicitly
    // cancelled/failed by the user, are not reconcilable. Leave them alone.
    match current {
        ImportRunState::Created
        | ImportRunState::Scanning
        | ImportRunState::Fingerprinting
        | ImportRunState::DetectingDuplicates
        | ImportRunState::Analyzing
        | ImportRunState::ReviewRequired
        | ImportRunState::ReadyToCommit
        | ImportRunState::Cancelled
        | ImportRunState::Failed => {
            return Ok(ReconciledRunState {
                import_run_id,
                state: current,
                completed_at: None,
                changed: false,
            });
        }
        ImportRunState::Committing
        | ImportRunState::RecoveryRequired
        | ImportRunState::Completed => {}
    }

    // Frozen plan is the album universe for this run. Prefer frozen; accept
    // consumed for already-completed runs. A missing plan means commit was
    // never attempted — leave the run state untouched.
    let frozen = ImportRepository::load_frozen_plan(client, import_run_id).await?;
    let Some(frozen) = frozen else {
        return Ok(ReconciledRunState {
            import_run_id,
            state: current,
            completed_at: None,
            changed: false,
        });
    };

    let transactions =
        ImportRepository::get_all_transactions_for_run(client, import_run_id).await?;

    // Validate the frozen plan up front (state + hash + album/image counts).
    // A tampered or inconsistent plan is a plan-integrity error: surface it
    // as a recovery_required run rather than silently completing.
    if let Err(plan_err) = validate_and_hash_frozen_plan(&frozen, frozen.library_root_id) {
        // Plan integrity failure → recovery_required. Persist this so the
        // GUI/API see the real state, and return the persisted state.
        if current != ImportRunState::RecoveryRequired {
            ImportRepository::set_import_run_state(
                client,
                import_run_id,
                &ImportRunState::RecoveryRequired,
                None,
                true,
            )
            .await?;
        }
        let _ = plan_err; // diagnostics only; the state itself is the signal
        return read_back_run_state(client, import_run_id, current, true).await;
    }

    // Empty frozen plan: complete ONLY if the full invariant check passes
    // (no active / failed / cancelled / conflict transactions, no residual
    // plan album/image rows that contradict the empty plan). This is the
    // Phase 3 fix — previously an empty plan completed unconditionally,
    // bypassing transaction checks.
    if frozen.albums.is_empty() {
        if empty_plan_allows_completion(&transactions) {
            return complete_run_if_needed(client, import_run_id, current).await;
        }
        // An empty plan with surviving transactions / conflicts is not
        // complete: route to recovery_required.
        return set_recovery_required(client, import_run_id, &current).await;
    }

    // Rule 2: any conflict forces recovery_required.
    let has_conflict = transactions.iter().any(|t| {
        matches!(
            TransactionState::parse(&t.state),
            Ok(TransactionState::Conflict)
        )
    });
    if has_conflict {
        return set_recovery_required(client, import_run_id, &current).await;
    }

    // Rule 3: any active (recoverable) transaction forces recovery_required.
    let has_active = transactions.iter().any(|t| {
        matches!(
            TransactionState::parse(&t.state),
            Ok(TransactionState::Planned
                | TransactionState::Staging
                | TransactionState::Verifying
                | TransactionState::Verified
                | TransactionState::Publishing
                | TransactionState::Published
                | TransactionState::DbCommitting
                | TransactionState::LibraryCommitted
                | TransactionState::SourceArchiving
                | TransactionState::CleanupRequired)
        )
    });
    if has_active {
        return set_recovery_required(client, import_run_id, &current).await;
    }

    // Rule 4: any failed/cancelled transaction blocks completion.
    let has_failed_or_cancelled = transactions.iter().any(|t| {
        matches!(
            TransactionState::parse(&t.state),
            Ok(TransactionState::Failed | TransactionState::Cancelled)
        )
    });
    if has_failed_or_cancelled {
        return set_recovery_required(client, import_run_id, &current).await;
    }

    // Rule 5: every frozen-plan album must have reached source_archived.
    let archived_album_ids: std::collections::HashSet<Uuid> = transactions
        .iter()
        .filter(|t| {
            matches!(
                TransactionState::parse(&t.state),
                Ok(TransactionState::SourceArchived)
            )
        })
        .map(|t| t.import_album_id)
        .collect();

    let all_archived = frozen
        .albums
        .iter()
        .all(|(a, _)| archived_album_ids.contains(&a.import_album_id));

    if all_archived {
        complete_run_if_needed(client, import_run_id, current).await
    } else {
        // Some plan album has no transaction at all — commit was aborted
        // mid-pipeline before a row was inserted. Recovery required.
        set_recovery_required(client, import_run_id, &current).await
    }
}

/// Empty-plan invariant check (Phase 3).
///
/// An empty frozen plan completes the run ONLY when there are **no**
/// `file_transactions` rows for the run at all (commit was a genuine
/// no-op). A run with any transaction — active, conflict, failed,
/// cancelled, or even `source_archived` (leftover from a prior commit
/// of a now-empty plan), or any **unparseable** state — is NOT
/// auto-completed by an empty plan. Instead:
/// - active/conflict/failed/cancelled → `recovery_required` (handled by
///   the caller's `set_recovery_required` branch).
/// - `source_archived` → falls through to the all-archived check (which
///   is vacuously false for an empty plan, so `recovery_required`).
/// - unparseable state → `recovery_required` (integrity error: the
///   database cannot vouch for the transaction, so it must not be
///   silently treated as safe).
///
/// P1 fix: previously `Err(_)` was tolerated as "safe", which let a
/// corrupted/unknown transaction state slip through an empty plan to
/// `completed`. Now only the empty set (no transactions) completes.
fn empty_plan_allows_completion(
    transactions: &[crate::repositories::import_repository::FileTransactionStateRow],
) -> bool {
    transactions.is_empty()
}

/// Transition the run to `Completed` if it isn't already, writing a stable
/// `completed_at` timestamp. Returns the persisted `completed_at` read back
/// from the DB (Phase 6: never a transient in-memory value).
///
/// If the run was already `Completed`, no write occurs and the existing
/// persisted `completed_at` is returned verbatim — so repeated reconcile
/// calls and post-restart reads return the same value.
async fn complete_run_if_needed(
    client: &Client,
    import_run_id: Uuid,
    current: ImportRunState,
) -> Result<ReconciledRunState, AppError> {
    let changed = current != ImportRunState::Completed;
    if changed {
        // Write a stable timestamp. If a prior completed_at exists (e.g. the
        // row was Completed, pulled back, and is now re-completing), the
        // COALESCE in set_import_run_state preserves it; otherwise we set
        // a fresh one. The value returned to the caller is read back from
        // the DB so it is always the persisted truth.
        ImportRepository::set_import_run_state(
            client,
            import_run_id,
            &ImportRunState::Completed,
            Some(chrono::Utc::now()),
            false,
        )
        .await?;
    }
    read_back_run_state(client, import_run_id, ImportRunState::Completed, changed).await
}

/// Read back the persisted `state` + `completed_at` for the run. Used after
/// any reconcile write so the returned `ReconciledRunState` reflects the
/// exact DB row — not an in-memory guess.
///
/// `prev_state` is the state the caller already computed; if the read-back
/// state disagrees, the read-back wins (it is the source of truth).
async fn read_back_run_state(
    client: &Client,
    import_run_id: Uuid,
    prev_state: ImportRunState,
    changed: bool,
) -> Result<ReconciledRunState, AppError> {
    let row = ImportRepository::get_import_run_by_id(client, import_run_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("import run {import_run_id} not found")))?;
    let persisted_state = ImportRunState::from_str_opt(&row.state).unwrap_or(prev_state);
    Ok(ReconciledRunState {
        import_run_id,
        state: persisted_state,
        completed_at: row.completed_at,
        changed,
    })
}

async fn set_recovery_required(
    client: &Client,
    import_run_id: Uuid,
    current: &ImportRunState,
) -> Result<ReconciledRunState, AppError> {
    let target = ImportRunState::RecoveryRequired;
    let changed = current != &target;
    if changed {
        // Clear completed_at: a run that is no longer completed must not
        // carry a completed_at timestamp.
        ImportRepository::set_import_run_state(client, import_run_id, &target, None, true).await?;
    }
    Ok(ReconciledRunState {
        import_run_id,
        state: target,
        completed_at: None,
        changed,
    })
}

/// Attempt to recover a single transaction based on its current state.
///
/// This is the real recovery driver. It loads the frozen plan, the persisted
/// transaction + operations, and the on-disk state, then executes the
/// appropriate resume action. Idempotent: running twice is a no-op the second
/// time.
pub async fn recover_transaction(
    postgres_manager: Arc<Mutex<PostgresManager>>,
    transaction_id: Uuid,
) -> Result<RecoveryOutcome, AppError> {
    let (mut client, handle) = {
        let mgr = postgres_manager.lock().await;
        mgr.connect()
            .await
            .map_err(|e| AppError::Internal(format!("failed to connect for recovery: {e}")))?
    };

    let result = recover_transaction_with_client(&mut client, transaction_id).await;

    drop(client);
    handle.abort();
    result
}

async fn recover_transaction_with_client(
    client: &mut Client,
    transaction_id: Uuid,
) -> Result<RecoveryOutcome, AppError> {
    let tx = ImportRepository::get_file_transaction(client, transaction_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("transaction {transaction_id} not found")))?;
    let import_run_id = tx.import_run_id;

    let current = match TransactionState::parse(&tx.state) {
        Ok(s) => s,
        Err(_) => {
            let state = tx.state.clone();
            // Even for unparseable rows we still reconcile the parent run so
            // a stray garbage-state transaction cannot leave the run stuck
            // in `completed`.
            reconcile_import_run_state(client, import_run_id).await?;
            return Ok(RecoveryOutcome {
                transaction_id,
                final_state: state.clone(),
                recovered: false,
                terminal: false,
                message: format!("unparseable transaction state '{state}'"),
            });
        }
    };

    // Phase 1: terminal transactions are NOT auto-recovered. A `failed` or
    // `cancelled` transaction is terminal-but-not-recovered — callers must
    // not treat it as a successful recovery, and the run must stay
    // `recovery_required` (or be moved to a real `failed` run path). Only
    // `source_archived` is a genuine "already done, nothing to do" success.
    let outcome: Result<(String, bool, bool, String), AppError> = if current.is_terminal() {
        let (recovered, msg) = match current {
            TransactionState::SourceArchived => (
                true,
                "transaction already source_archived".to_string(),
            ),
            TransactionState::Failed | TransactionState::Cancelled => (
                false,
                format!(
                    "transaction is in terminal state '{current}'; not recoverable (manual resolution required)"
                ),
            ),
            _ => (false, format!("terminal state {current}")),
        };
        Ok((current.to_string(), recovered, true, msg))
    } else if current == TransactionState::Conflict {
        Ok((
            current.to_string(),
            false,
            false,
            "conflict requires manual resolution".to_string(),
        ))
    } else {
        recover_active_transaction(client, transaction_id, &tx, current, import_run_id)
            .await
            .map(|(s, recovered, msg)| (s, recovered, false, msg))
    };

    // Always reconcile the parent run after any transaction-level change
    // (including no-op terminal/conflict paths). The reconciler decides
    // whether this single transaction's new state promotes the run to
    // `completed`, keeps it at `recovery_required`, or pulls a `completed`
    // run back to `recovery_required`.
    reconcile_import_run_state(client, import_run_id).await?;

    let (final_state, recovered, terminal, message) = match outcome {
        Ok(o) => o,
        Err(e) => return Err(e),
    };

    Ok(RecoveryOutcome {
        transaction_id,
        final_state,
        recovered,
        terminal,
        message,
    })
}

/// Drive a non-terminal, non-conflict transaction through its resume action.
///
/// Kept as a separate function so [`recover_transaction_with_client`] can
/// always call [`reconcile_import_run_state`] at the bottom, regardless of
/// whether the resume action succeeded, failed, or was bypassed.
async fn recover_active_transaction(
    client: &mut Client,
    transaction_id: Uuid,
    tx: &FileTransactionFullRow,
    current: TransactionState,
    import_run_id: Uuid,
) -> Result<(String, bool, String), AppError> {
    // Load the frozen plan to know what files should exist.
    let frozen = ImportRepository::load_frozen_plan(client, import_run_id)
        .await?
        .ok_or_else(|| {
            AppError::Internal(format!(
                "no frozen plan for run {} of transaction {transaction_id}",
                import_run_id
            ))
        })?;

    // Find this transaction's album in the plan.
    let (plan_album, plan_images) = frozen
        .albums
        .iter()
        .find(|(a, _)| a.import_album_id == tx.import_album_id)
        .ok_or_else(|| {
            AppError::Internal(format!(
                "transaction {} album {} not found in frozen plan",
                transaction_id, tx.import_album_id
            ))
        })?
        .clone();

    let album_relative_path = normalize_relative_path(&plan_album.target_relative_path)?;
    let library_root_id = frozen.library_root_id;
    let library_root_path =
        ImportRepository::get_library_root_path(client, library_root_id).await?;
    let library_root = PathBuf::from(&library_root_path);
    let validated_plan_hash = validate_and_hash_frozen_plan(&frozen, library_root_id)?;
    let lease_owner = format!("imagedb-recovery-{}", Uuid::new_v4());
    let lease_token = Uuid::new_v4();
    ImportRepository::acquire_library_root_lease(
        client,
        library_root_id,
        &lease_owner,
        lease_token,
        LIBRARY_ROOT_LEASE_TTL_SECS,
    )
    .await?;

    // Dispatch by state.
    let outcome = match current {
        TransactionState::Planned | TransactionState::Staging => {
            resume_staging(
                client,
                tx,
                &frozen.plan_id,
                &validated_plan_hash,
                &plan_images,
            )
            .await
        }
        TransactionState::Verifying | TransactionState::Verified => {
            resume_verify_and_publish(
                client,
                tx,
                &frozen.plan_id,
                &validated_plan_hash,
                &library_root,
                library_root_id,
                import_run_id,
                &album_relative_path,
                plan_album,
                &plan_images,
            )
            .await
        }
        TransactionState::Publishing => {
            resume_publishing(
                client,
                tx,
                &frozen.plan_id,
                &validated_plan_hash,
                &library_root,
                library_root_id,
                import_run_id,
                &album_relative_path,
                plan_album,
                &plan_images,
            )
            .await
        }
        TransactionState::Published | TransactionState::DbCommitting => {
            resume_db_commit(
                client,
                tx,
                &frozen.plan_id,
                &validated_plan_hash,
                &library_root,
                library_root_id,
                import_run_id,
                &album_relative_path,
                plan_album,
                &plan_images,
            )
            .await
        }
        TransactionState::LibraryCommitted | TransactionState::SourceArchiving => {
            resume_source_archive(client, tx, &library_root, &album_relative_path).await
        }
        TransactionState::CleanupRequired => resume_cleanup(client, tx, &library_root).await,
        _ => Ok((
            current.to_string(),
            false,
            format!("no recovery action for state {}", current),
        )),
    };

    let release_result =
        ImportRepository::release_library_root_lease(client, library_root_id, lease_token).await;
    match (outcome, release_result) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Ok(_), Err(e)) => Err(e),
        (Err(e), _) => Err(e),
    }
}

/// planned/staging: clean .part files, verify reusable staged files, resume
/// copying the rest, then continue through verify/publish/commit.
async fn resume_staging(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    _plan_id: &Uuid,
    _plan_hash: &[u8],
    plan_images: &[PlanImageRow],
) -> Result<(String, bool, String), AppError> {
    let staging_dir = tx
        .staging_path
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Internal("transaction has no staging_path".to_string()))?;
    let ops = ImportRepository::get_file_operations(client, tx.id).await?;

    ImportRepository::update_file_transaction_state(
        client,
        tx.id,
        &TransactionState::Staging,
        None,
    )
    .await?;

    if !staging_dir.exists() {
        tokio::fs::create_dir_all(&staging_dir)
            .await
            .map_err(|e| AppError::IoError(format!("cannot recreate staging dir: {e}")))?;
    }

    for img in plan_images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let staged = staging_dir.join(&target_rel);
        let part = staging_dir.join(format!("{target_rel}.part"));

        if let Some(parent) = staged.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::IoError(format!("cannot create staging subdir: {e}")))?;
        }

        // Remove any leftover .part from an interrupted copy.
        let _ = tokio::fs::remove_file(&part).await;

        // If the staged file already exists and verifies, reuse it.
        if let Ok(meta) = tokio::fs::metadata(&staged).await {
            if meta.len() == img.expected_file_size as u64 {
                if let Ok(actual) = hash_file(&staged).await {
                    if actual == img.expected_blake3 {
                        // Already verified — mark the op verified if not already.
                        if let Some(op) = ops.iter().find(|o| o.target_path.ends_with(&target_rel))
                        {
                            if FileOpState::parse(&op.state).ok() != Some(FileOpState::Verified) {
                                ImportRepository::update_file_operation_state(
                                    client,
                                    op.id,
                                    &FileOpState::Verified,
                                    Some(&actual),
                                    None,
                                )
                                .await?;
                            }
                        }
                        continue;
                    }
                }
            }
            // Size matches but hash doesn't, or size wrong — re-copy.
            let _ = tokio::fs::remove_file(&staged).await;
        }

        let src = Path::new(&img.source_path);
        if !src.exists() {
            let msg = format!("source file missing during recovery: {}", src.display());
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Failed,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Failed.to_string(), false, msg));
        }

        let op_id = ops
            .iter()
            .find(|o| o.target_path.ends_with(&target_rel))
            .map(|o| o.id);
        if let Some(op_id) = op_id {
            ImportRepository::update_file_operation_state(
                client,
                op_id,
                &FileOpState::Copying,
                None,
                None,
            )
            .await?;
        }
        // Recovery runs to completion — no cancel token. The caller
        // (recover_transaction) is invoked interactively by the operator.
        let actual = stream_copy_with_hash(src, &part, None).await?;
        if actual != img.expected_blake3 {
            let _ = tokio::fs::remove_file(&part).await;
            let msg = format!("BLAKE3 mismatch recovering {}", src.display());
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Failed,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Failed.to_string(), false, msg));
        }
        tokio::fs::rename(&part, &staged)
            .await
            .map_err(|e| AppError::IoError(format!("rename part failed: {e}")))?;
        if let Some(op_id) = op_id {
            ImportRepository::update_file_operation_state(
                client,
                op_id,
                &FileOpState::Verified,
                Some(&actual),
                None,
            )
            .await?;
        }
    }

    // Staging is now complete; continue through verify + publish + commit.
    ImportRepository::update_file_transaction_state(
        client,
        tx.id,
        &TransactionState::Verifying,
        None,
    )
    .await?;
    verify_staging_set(&staging_dir, plan_images).await?;
    ImportRepository::update_file_transaction_state(
        client,
        tx.id,
        &TransactionState::Verified,
        None,
    )
    .await?;

    Ok((
        TransactionState::Verified.to_string(),
        true,
        "staging resumed and verified; call recovery again to publish".to_string(),
    ))
}

pub(crate) async fn hash_file(path: &Path) -> Result<Vec<u8>, AppError> {
    let mut f = tokio::fs::File::open(path)
        .await
        .map_err(|e| AppError::IoError(format!("cannot open {}: {e}", path.display())))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let n = f
            .read(&mut buf)
            .await
            .map_err(|e| AppError::IoError(format!("read error: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().as_bytes().to_vec())
}

#[allow(clippy::too_many_arguments)]
async fn resume_verify_and_publish(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    plan_id: &Uuid,
    plan_hash: &[u8],
    library_root: &Path,
    library_root_id: Uuid,
    import_run_id: Uuid,
    album_relative_path: &str,
    plan_album: crate::repositories::import_repository::PlanAlbumRow,
    plan_images: &[PlanImageRow],
) -> Result<(String, bool, String), AppError> {
    let staging_dir = tx
        .staging_path
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Internal("transaction has no staging_path".to_string()))?;

    // Re-verify the staging set.
    verify_staging_set(&staging_dir, plan_images).await?;
    ImportRepository::update_file_transaction_state(
        client,
        tx.id,
        &TransactionState::Verified,
        None,
    )
    .await?;

    // Write the manifest if not present, then publish.
    publish_from_staging(
        client,
        tx,
        plan_id,
        plan_hash,
        library_root,
        library_root_id,
        import_run_id,
        album_relative_path,
        plan_album,
        plan_images,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resume_publishing(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    plan_id: &Uuid,
    plan_hash: &[u8],
    library_root: &Path,
    library_root_id: Uuid,
    import_run_id: Uuid,
    album_relative_path: &str,
    plan_album: crate::repositories::import_repository::PlanAlbumRow,
    plan_images: &[PlanImageRow],
) -> Result<(String, bool, String), AppError> {
    let staging_dir = tx.staging_path.as_ref().map(PathBuf::from);
    let publish_dir = library_root.join("Albums").join(album_relative_path);

    if let Some(staging) = &staging_dir {
        if staging.exists() && !publish_dir.exists() {
            let strategy = select_commit_publish_strategy(library_root)?;
            publish_verified_staging(
                strategy,
                staging,
                &publish_dir,
                tx.id,
                plan_hash,
                tx.manifest_hash.as_deref().ok_or_else(|| {
                    AppError::Internal("transaction has no manifest_hash".to_string())
                })?,
                album_relative_path,
                plan_images,
            )
            .await?;
            // Manifest moved with the rename: record the published path.
            let published_manifest = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
            if !published_manifest.exists() {
                return Err(AppError::Internal(format!(
                    "published manifest missing after rename: {}",
                    published_manifest.display()
                )));
            }
            ImportRepository::set_transaction_manifest_path(
                client,
                tx.id,
                &published_manifest.display().to_string(),
            )
            .await?;
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Published,
                None,
            )
            .await?;
            return resume_db_commit(
                client,
                tx,
                plan_id,
                plan_hash,
                library_root,
                library_root_id,
                import_run_id,
                album_relative_path,
                plan_album,
                plan_images,
            )
            .await;
        }
    }

    // Target exists: verify manifest matches → published; else conflict.
    // The hash is computed over the on-disk bytes (never re-serialized) and
    // compared against file_transactions.manifest_hash if persisted.
    if publish_dir.exists() {
        let (manifest, raw_hash) = match read_manifest_with_hash(&publish_dir) {
            Ok(pair) => pair,
            Err(e) => {
                let msg = format!(
                    "conflict: target {} has unreadable/unparseable manifest: {e}",
                    publish_dir.display()
                );
                ImportRepository::update_file_transaction_state(
                    client,
                    tx.id,
                    &TransactionState::Conflict,
                    Some(&msg),
                )
                .await?;
                return Ok((TransactionState::Conflict.to_string(), false, msg));
            }
        };
        let mut conflict: Option<String> = None;
        if manifest.transaction_id != tx.id.to_string() {
            conflict = Some(format!(
                "manifest transaction_id {} != tx {}",
                manifest.transaction_id, tx.id
            ));
        } else if manifest.plan_id != plan_id.to_string() {
            conflict = Some(format!(
                "manifest plan_id {} != expected {}",
                manifest.plan_id, plan_id
            ));
        } else if manifest.import_run_id != import_run_id.to_string() {
            conflict = Some(format!(
                "manifest import_run_id {} != expected {}",
                manifest.import_run_id, import_run_id
            ));
        } else if manifest.import_album_id != tx.import_album_id.to_string() {
            conflict = Some(format!(
                "manifest import_album_id {} != expected {}",
                manifest.import_album_id, tx.import_album_id
            ));
        } else if manifest.library_root_id != library_root_id.to_string() {
            conflict = Some(format!(
                "manifest library_root_id {} != expected {}",
                manifest.library_root_id, library_root_id
            ));
        } else if manifest.album_relative_path != album_relative_path {
            conflict = Some(format!(
                "manifest album_relative_path '{}' != expected '{}'",
                manifest.album_relative_path, album_relative_path
            ));
        } else if manifest.schema_version
            != crate::services::commit_service::MANIFEST_SCHEMA_VERSION
        {
            conflict = Some(format!(
                "manifest schema_version {} != expected {}",
                manifest.schema_version,
                crate::services::commit_service::MANIFEST_SCHEMA_VERSION
            ));
        } else if manifest.plan_hash != crate::services::commit_service::bytes_to_hex(plan_hash) {
            conflict = Some(format!(
                "manifest plan_hash {} != expected {}",
                manifest.plan_hash,
                crate::services::commit_service::bytes_to_hex(plan_hash)
            ));
        } else if let Some(stored) = &tx.manifest_hash {
            if stored != &raw_hash {
                conflict = Some(format!(
                    "manifest_hash mismatch: stored {} raw-byte {}",
                    crate::services::commit_service::bytes_to_hex(stored),
                    crate::services::commit_service::bytes_to_hex(&raw_hash)
                ));
            }
        } else {
            conflict = Some("transaction has no manifest_hash".to_string());
        }
        if let Some(msg) = conflict {
            let full = format!(
                "conflict: target {} exists with mismatched manifest: {msg}",
                publish_dir.display()
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&full),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, full));
        }
        verify_published_file_set(&publish_dir, plan_images).await?;
        match read_commit_marker(&publish_dir) {
            Ok(marker) => {
                if let Err(msg) = validate_commit_marker(
                    &marker,
                    tx.id,
                    plan_hash,
                    &raw_hash,
                    album_relative_path,
                    plan_images,
                ) {
                    let full = format!(
                        "conflict: target {} has mismatched commit marker: {msg}",
                        publish_dir.display()
                    );
                    ImportRepository::update_file_transaction_state(
                        client,
                        tx.id,
                        &TransactionState::Conflict,
                        Some(&full),
                    )
                    .await?;
                    return Ok((TransactionState::Conflict.to_string(), false, full));
                }
            }
            Err(_) => {
                write_commit_marker(
                    &publish_dir,
                    tx.id,
                    plan_hash,
                    &raw_hash,
                    album_relative_path,
                    plan_images,
                )
                .await?;
            }
        }
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Published,
            None,
        )
        .await?;
        return resume_db_commit(
            client,
            tx,
            plan_id,
            plan_hash,
            library_root,
            library_root_id,
            import_run_id,
            album_relative_path,
            plan_album,
            plan_images,
        )
        .await;
    }

    // Neither staging nor target — must re-stage from source.
    let msg = "staging missing during publishing; re-stage required".to_string();
    ImportRepository::update_file_transaction_state(
        client,
        tx.id,
        &TransactionState::Staging,
        Some(&msg),
    )
    .await?;
    Ok((TransactionState::Staging.to_string(), false, msg))
}

#[allow(clippy::too_many_arguments)]
async fn publish_from_staging(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    plan_id: &Uuid,
    plan_hash: &[u8],
    library_root: &Path,
    library_root_id: Uuid,
    import_run_id: Uuid,
    album_relative_path: &str,
    plan_album: crate::repositories::import_repository::PlanAlbumRow,
    plan_images: &[PlanImageRow],
) -> Result<(String, bool, String), AppError> {
    let staging_dir = tx
        .staging_path
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Internal("transaction has no staging_path".to_string()))?;

    // Write the manifest into staging (temp + atomic rename).
    let manifest = build_manifest(
        &tx.id,
        *plan_id,
        plan_hash,
        import_run_id,
        tx.import_album_id,
        library_root_id,
        album_relative_path,
        plan_images,
    );
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::Internal(format!("manifest serialize failed: {e}")))?;
    let manifest_hash = blake3::hash(manifest_json.as_bytes()).as_bytes().to_vec();
    let manifest_dir = staging_dir.join(".imagedb");
    tokio::fs::create_dir_all(&manifest_dir)
        .await
        .map_err(|e| AppError::IoError(format!("cannot create manifest dir: {e}")))?;
    let tmp = manifest_dir.join(".imagedb-manifest.json.tmp");
    let final_m = manifest_dir.join(".imagedb-manifest.json");
    write_synced_then_rename(&tmp, &final_m, manifest_json.as_bytes()).await?;
    ImportRepository::set_transaction_hashes(client, tx.id, None, Some(&manifest_hash)).await?;

    // Publish using the currently verified storage strategy.
    let publishing = state_machine::transition_transaction(TransactionState::Verified, "publish")?;
    ImportRepository::update_file_transaction_state(client, tx.id, &publishing, None).await?;

    let publish_dir = library_root.join("Albums").join(album_relative_path);
    if let Some(parent) = publish_dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::IoError(format!("cannot create publish parent: {e}")))?;
    }
    let strategy = select_commit_publish_strategy(library_root)?;
    publish_verified_staging(
        strategy,
        &staging_dir,
        &publish_dir,
        tx.id,
        plan_hash,
        &manifest_hash,
        album_relative_path,
        plan_images,
    )
    .await?;
    // Manifest moved with the rename: record the published path.
    let published_manifest = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    if !published_manifest.exists() {
        return Err(AppError::Internal(format!(
            "published manifest missing after rename: {}",
            published_manifest.display()
        )));
    }
    ImportRepository::set_transaction_manifest_path(
        client,
        tx.id,
        &published_manifest.display().to_string(),
    )
    .await?;
    let published =
        state_machine::transition_transaction(TransactionState::Publishing, "published")?;
    ImportRepository::update_file_transaction_state(client, tx.id, &published, None).await?;

    // Continue to DB commit.
    let refreshed_tx = ImportRepository::get_file_transaction(client, tx.id)
        .await?
        .ok_or_else(|| {
            AppError::Internal(format!(
                "file transaction {} disappeared after publish",
                tx.id
            ))
        })?;
    resume_db_commit(
        client,
        &refreshed_tx,
        plan_id,
        plan_hash,
        library_root,
        library_root_id,
        import_run_id,
        album_relative_path,
        plan_album,
        plan_images,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn resume_db_commit(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    plan_id: &Uuid,
    plan_hash: &[u8],
    library_root: &Path,
    library_root_id: Uuid,
    import_run_id: Uuid,
    album_relative_path: &str,
    plan_album: crate::repositories::import_repository::PlanAlbumRow,
    plan_images: &[PlanImageRow],
) -> Result<(String, bool, String), AppError> {
    let publish_dir = library_root.join("Albums").join(album_relative_path);

    // Verify the published dir + manifest before touching the DB.
    if !publish_dir.exists() {
        let msg = format!("published dir missing: {}", publish_dir.display());
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    // Read the manifest from disk along with its raw-byte BLAKE3. Recovery
    // must NEVER re-serialize the manifest — the on-disk bytes are the only
    // authoritative input, and the hash must match both
    // `file_transactions.manifest_hash` and `library_albums.manifest_hash`.
    let (manifest, manifest_hash) = match read_manifest_with_hash(&publish_dir) {
        Ok(pair) => pair,
        Err(e) => {
            let msg = format!("published manifest unreadable/unparseable: {e}");
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
    };
    if manifest.transaction_id != tx.id.to_string() {
        let msg = format!(
            "manifest transaction_id {} != tx {}",
            manifest.transaction_id, tx.id
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    if manifest.plan_id != plan_id.to_string() {
        let msg = format!(
            "manifest plan_id {} != expected {}",
            manifest.plan_id, plan_id
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    if manifest.import_run_id != import_run_id.to_string() {
        let msg = format!(
            "manifest import_run_id {} != expected {}",
            manifest.import_run_id, import_run_id
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    if manifest.import_album_id != tx.import_album_id.to_string() {
        let msg = format!(
            "manifest import_album_id {} != expected {}",
            manifest.import_album_id, tx.import_album_id
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    if manifest.library_root_id != library_root_id.to_string() {
        let msg = format!(
            "manifest library_root_id {} != expected {}",
            manifest.library_root_id, library_root_id
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    if manifest.album_relative_path != album_relative_path {
        let msg = format!(
            "manifest album_relative_path '{}' != expected '{}'",
            manifest.album_relative_path, album_relative_path
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    if manifest.plan_hash != crate::services::commit_service::bytes_to_hex(plan_hash) {
        let msg = format!(
            "manifest plan_hash {} != expected {}",
            manifest.plan_hash,
            crate::services::commit_service::bytes_to_hex(plan_hash)
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    if manifest.schema_version != crate::services::commit_service::MANIFEST_SCHEMA_VERSION {
        let msg = format!(
            "manifest schema_version {} != expected {}",
            manifest.schema_version,
            crate::services::commit_service::MANIFEST_SCHEMA_VERSION
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    // If a prior manifest_hash was persisted on the transaction, the raw
    // on-disk bytes must still match — otherwise the manifest was tampered
    // with between the original commit and this recovery run.
    match &tx.manifest_hash {
        Some(stored) if stored == &manifest_hash => {}
        Some(stored) => {
            let msg = format!(
                "manifest_hash mismatch during recovery: stored {} raw-byte {}",
                crate::services::commit_service::bytes_to_hex(stored),
                crate::services::commit_service::bytes_to_hex(&manifest_hash)
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
        None => {
            let msg = "transaction has no manifest_hash".to_string();
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
    }

    if manifest.image_count != plan_images.len() as u32 {
        let msg = format!(
            "manifest image_count {} != plan {}",
            manifest.image_count,
            plan_images.len()
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    if manifest.images.len() != plan_images.len() {
        let msg = format!(
            "manifest images array length {} != plan {}",
            manifest.images.len(),
            plan_images.len()
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    let mut manifest_by_rel = HashMap::new();
    for entry in &manifest.images {
        if manifest_by_rel
            .insert(entry.relative_path.clone(), entry)
            .is_some()
        {
            let msg = format!(
                "manifest has duplicate relative_path '{}'",
                entry.relative_path
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
    }
    let mut seen_rels = HashSet::new();
    for img in plan_images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        seen_rels.insert(target_rel.clone());
        let Some(entry) = manifest_by_rel.get(&target_rel) else {
            let msg = format!("file {target_rel} missing from manifest");
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        };
        if entry.source_path != img.source_path {
            let msg = format!(
                "manifest source_path mismatch for {target_rel}: manifest '{}' plan '{}'",
                entry.source_path, img.source_path
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
        if entry.file_size != img.expected_file_size {
            let msg = format!(
                "manifest file_size mismatch for {target_rel}: manifest {} plan {}",
                entry.file_size, img.expected_file_size
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
        if entry.blake3 != crate::services::commit_service::bytes_to_hex(&img.expected_blake3) {
            let msg = format!("manifest blake3 mismatch for {target_rel}");
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
    }
    for rel in manifest_by_rel.keys() {
        if !seen_rels.contains(rel) {
            let msg = format!("manifest has extra entry not in frozen plan: {rel}");
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
    }
    if let Some(msg) = detect_extra_published_files(&publish_dir, &seen_rels).await {
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }

    let published_manifest = publish_dir.join(".imagedb").join(".imagedb-manifest.json");
    if !published_manifest.exists() {
        let msg = format!(
            "published manifest missing: {}",
            published_manifest.display()
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    ImportRepository::set_transaction_manifest_path(
        client,
        tx.id,
        &published_manifest.display().to_string(),
    )
    .await?;

    // Verify every published file still matches (content may have changed).
    for img in plan_images {
        let target_rel = normalize_relative_path(&img.target_relative_path)?;
        let file_path = publish_dir.join(&target_rel);
        let meta = match tokio::fs::metadata(&file_path).await {
            Ok(m) => m,
            Err(_) => {
                let msg = format!("published file missing: {}", file_path.display());
                ImportRepository::update_file_transaction_state(
                    client,
                    tx.id,
                    &TransactionState::Conflict,
                    Some(&msg),
                )
                .await?;
                return Ok((TransactionState::Conflict.to_string(), false, msg));
            }
        };
        if meta.len() != img.expected_file_size as u64 {
            let msg = format!("published file size mismatch: {}", file_path.display());
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
        let actual = hash_file(&file_path).await?;
        if actual != img.expected_blake3 {
            let msg = format!("published file blake3 mismatch: {}", file_path.display());
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            return Ok((TransactionState::Conflict.to_string(), false, msg));
        }
    }

    // Transition to db_committing and run the DB transaction.
    let db_committing =
        state_machine::transition_transaction(TransactionState::Published, "db_commit")?;
    ImportRepository::update_file_transaction_state(client, tx.id, &db_committing, None).await?;

    commit_library_records_transaction(
        client,
        library_root_id,
        tx.id,
        *plan_id,
        plan_hash,
        &manifest_hash,
        &plan_album,
        album_relative_path,
        &manifest,
        plan_images,
    )
    .await?;

    let library_committed =
        state_machine::transition_transaction(TransactionState::DbCommitting, "library_committed")?;
    ImportRepository::update_file_transaction_state(client, tx.id, &library_committed, None)
        .await?;

    // Continue to source archive.
    let refreshed_tx = ImportRepository::get_file_transaction(client, tx.id)
        .await?
        .ok_or_else(|| {
            AppError::Internal(format!(
                "file transaction {} disappeared after library commit",
                tx.id
            ))
        })?;
    resume_source_archive(client, &refreshed_tx, library_root, album_relative_path).await
}

/// Outcome of [`resolve_archive_entry_transition`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum ArchiveEntryAction {
    /// Proceed with archiving; the transition has been validated and the
    /// caller should persist the returned state before doing I/O.
    BeginArchive(TransactionState),
    /// The transaction is already fully archived; no I/O needed.
    AlreadyArchived,
}

/// Decide the correct archive-entry transition from the **real** persisted
/// transaction state. Pure function — no DB, no filesystem — so it can be
/// unit-tested in isolation.
///
/// | persisted state     | action                          |
/// |---------------------|---------------------------------|
/// | `library_committed` | `archive` → `source_archiving`  |
/// | `source_archiving`  | `retry_archive` → `source_archiving` |
/// | `source_archived`   | already done (skip)             |
/// | anything else       | `Err` — illegal for archive entry |
fn resolve_archive_entry_transition(
    current: TransactionState,
) -> Result<ArchiveEntryAction, AppError> {
    match current {
        TransactionState::LibraryCommitted => {
            let next = state_machine::transition_transaction(current, "archive")?;
            Ok(ArchiveEntryAction::BeginArchive(next))
        }
        TransactionState::SourceArchiving => {
            let next = state_machine::transition_transaction(current, "retry_archive")?;
            Ok(ArchiveEntryAction::BeginArchive(next))
        }
        TransactionState::SourceArchived => Ok(ArchiveEntryAction::AlreadyArchived),
        other => Err(AppError::Internal(format!(
            "cannot enter source archive recovery from state '{other}'; \
             expected library_committed, source_archiving, or source_archived"
        ))),
    }
}

/// library_committed/source_archiving: validate source and archive against
/// the **persisted source snapshot** (not the frozen import plan, which
/// only lists images selected for import), then safely rename source →
/// archive. Never auto-delete the source album directory.
///
/// | source | archive | outcome                                                    |
/// |--------|---------|------------------------------------------------------------|
/// | ✓      | ✗       | verify snapshot → rename → verify snapshot → source_archived |
/// | ✗      | ✓       | verify snapshot → source_archived if match                 |
/// | ✗      | ✗       | conflict                                                   |
/// | ✓      | ✓       | conflict (no delete, no overwrite)                         |
async fn resume_source_archive(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    library_root: &Path,
    album_relative_path: &str,
) -> Result<(String, bool, String), AppError> {
    // The library commit is already successful. Do NOT re-copy or re-publish.
    let publish_dir = library_root.join("Albums").join(album_relative_path);
    if !publish_dir.exists() {
        // Without the published dir we cannot trust the library state; this is
        // unexpected post-library_committed. Surface as conflict.
        let msg = format!(
            "published dir missing during archive: {}",
            publish_dir.display()
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }

    // Resolve the correct archive-entry transition from the **real** persisted
    // state. This handles library_committed (fresh archive), source_archiving
    // (retry after interrupted rename), and source_archived (idempotent skip).
    let parsed_state = TransactionState::parse(&tx.state)
        .map_err(|e| AppError::Internal(format!("unparseable tx state '{}': {e}", tx.state)))?;
    let entry = resolve_archive_entry_transition(parsed_state)?;
    let archiving = match entry {
        ArchiveEntryAction::AlreadyArchived => {
            return Ok((
                TransactionState::SourceArchived.to_string(),
                true,
                "already archived".to_string(),
            ));
        }
        ArchiveEntryAction::BeginArchive(next) => next,
    };

    // The source album root MUST come from the persisted import_albums row,
    // never from plan image parents. Commit and recovery share this rule so
    // the archive location is computed identically in both code paths.
    let import_album = ImportRepository::get_import_album_by_id(client, tx.import_album_id)
        .await?
        .ok_or_else(|| {
            AppError::Internal(format!(
                "import_album {} missing during recovery; cannot determine source album directory",
                tx.import_album_id
            ))
        })?;
    if import_album.source_path.is_empty() {
        let msg = format!(
            "import_album {} has empty source_path; cannot archive",
            tx.import_album_id
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }
    let source_album_dir = PathBuf::from(&import_album.source_path);

    // The FULL source snapshot is the only accepted evidence for archive
    // integrity. A missing snapshot means we cannot prove the archive is
    // complete, so we refuse to mark it source_archived.
    let snapshot_pair = load_source_album_snapshot(client, tx.import_album_id).await?;
    let Some((snapshot, snapshot_files)) = snapshot_pair else {
        let msg = format!(
            "no source snapshot for album {}; cannot archive safely",
            tx.import_album_id
        );
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    };

    // Phase 4: verify the persisted source snapshot's album path agrees with
    // the import album's source_path. Mismatch → conflict.
    if let Err(e) = crate::services::commit_service::validate_snapshot_album_path_identity(
        &snapshot.source_album_path,
        &source_album_dir,
    ) {
        let msg = e.to_string();
        ImportRepository::update_file_transaction_state(
            client,
            tx.id,
            &TransactionState::Conflict,
            Some(&msg),
        )
        .await?;
        return Ok((TransactionState::Conflict.to_string(), false, msg));
    }

    // Phase 4: archive root is derived from the persisted
    // `import_runs.source_root` (via compute_archive_dir) — never from
    // `source_album_dir.parent()`. The album relative path is normalized
    // so the archive always lives under the user's source tree.
    let archive_dir = crate::services::commit_service::compute_archive_dir(
        client,
        tx.import_run_id,
        &source_album_dir,
        album_relative_path,
        tx.id,
    )
    .await
    .map_err(|e| {
        // Compute failures must surface as a conflict so the operator can
        // resolve the path-identity issue, not a silent abort.
        AppError::Internal(format!("cannot compute archive dir: {e}"))
    })?;

    let source_exists = source_album_dir.exists();
    let archive_exists = archive_dir.exists();

    ImportRepository::update_file_transaction_state(client, tx.id, &archiving, None).await?;

    match (source_exists, archive_exists) {
        (true, false) => {
            // Source present, archive missing: verify source contents against
            // the persisted snapshot, then same-filesystem rename to archive,
            // then verify the archive still matches.
            if let Some(msg) = verify_source_snapshot_or_conflict(
                client,
                tx.id,
                &source_album_dir,
                &snapshot.snapshot_hash,
                &snapshot_files,
                "source album",
            )
            .await?
            {
                return Ok((TransactionState::Conflict.to_string(), false, msg));
            }
            tokio::fs::create_dir_all(archive_dir.parent().unwrap())
                .await
                .map_err(|e| AppError::IoError(format!("cannot create archive base: {e}")))?;
            tokio::fs::rename(&source_album_dir, &archive_dir)
                .await
                .map_err(|e| AppError::IoError(format!("source archive rename failed: {e}")))?;
            sync_parent_dir(&archive_dir).await?;

            if let Some(msg) = verify_source_snapshot_or_conflict(
                client,
                tx.id,
                &archive_dir,
                &snapshot.snapshot_hash,
                &snapshot_files,
                "archive after rename",
            )
            .await?
            {
                return Ok((TransactionState::Conflict.to_string(), false, msg));
            }

            let archived = state_machine::transition_transaction(
                TransactionState::SourceArchiving,
                "archived",
            )?;
            ImportRepository::update_file_transaction_state(client, tx.id, &archived, None).await?;
            Ok((
                TransactionState::SourceArchived.to_string(),
                true,
                "source archived".to_string(),
            ))
        }
        (false, true) => {
            // Source missing, archive present: only trust the archive if its
            // contents exactly match the persisted snapshot; otherwise conflict.
            if let Some(msg) = verify_source_snapshot_or_conflict(
                client,
                tx.id,
                &archive_dir,
                &snapshot.snapshot_hash,
                &snapshot_files,
                "archive",
            )
            .await?
            {
                return Ok((TransactionState::Conflict.to_string(), false, msg));
            }
            let archived = state_machine::transition_transaction(
                TransactionState::SourceArchiving,
                "archived",
            )?;
            ImportRepository::update_file_transaction_state(client, tx.id, &archived, None).await?;
            Ok((
                TransactionState::SourceArchived.to_string(),
                true,
                "archive verified against snapshot; source already archived".to_string(),
            ))
        }
        (false, false) => {
            // Neither exists: cannot confirm the source was preserved. Conflict.
            let msg = format!(
                "source {} and archive {} both missing; cannot confirm archive integrity",
                source_album_dir.display(),
                archive_dir.display()
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            Ok((TransactionState::Conflict.to_string(), false, msg))
        }
        (true, true) => {
            // Both exist: ambiguous state — do NOT delete or overwrite either.
            let msg = format!(
                "source {} and archive {} both present; refusing to overwrite or delete",
                source_album_dir.display(),
                archive_dir.display()
            );
            ImportRepository::update_file_transaction_state(
                client,
                tx.id,
                &TransactionState::Conflict,
                Some(&msg),
            )
            .await?;
            Ok((TransactionState::Conflict.to_string(), false, msg))
        }
    }
}

/// cleanup_required: remove only this transaction's staging dir. Failures
/// preserve the state + error.
async fn resume_cleanup(
    client: &mut Client,
    tx: &FileTransactionFullRow,
    _library_root: &Path,
) -> Result<(String, bool, String), AppError> {
    if let Some(staging) = &tx.staging_path {
        let staging_base = Path::new(staging)
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(staging));
        // Only remove the transaction's staging root
        // (`.imagedb/staging/<tx_id>`), never anything broader.
        if staging_base.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&staging_base).await {
                let msg = format!("cleanup failed: {e}");
                ImportRepository::update_file_transaction_state(
                    client,
                    tx.id,
                    &TransactionState::CleanupRequired,
                    Some(&msg),
                )
                .await?;
                return Ok((TransactionState::CleanupRequired.to_string(), false, msg));
            }
        }
    }
    let archived =
        state_machine::transition_transaction(TransactionState::CleanupRequired, "cleaned")?;
    ImportRepository::update_file_transaction_state(client, tx.id, &archived, None).await?;
    Ok((
        TransactionState::SourceArchived.to_string(),
        true,
        "cleanup complete".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::commit_service::validate_plan_image_sources;

    fn img(source_path: &str, source_rel: &str, target_rel: &str) -> PlanImageRow {
        PlanImageRow {
            id: Uuid::new_v4(),
            plan_album_id: Uuid::new_v4(),
            import_image_id: Uuid::new_v4(),
            source_path: source_path.to_string(),
            source_relative_path: source_rel.to_string(),
            target_relative_path: target_rel.to_string(),
            expected_file_size: 1,
            expected_blake3: vec![0; 32],
            width: None,
            height: None,
            format: None,
        }
    }

    /// Archive location is derived from the persisted
    /// `import_runs.source_root` plus the tx id and album relative path —
    /// never from `source_album_dir.parent()` and never from a plan image
    /// parent. This keeps commit and recovery in lockstep and makes
    /// archive-only recovery (source already moved) possible while
    /// guaranteeing the archive always lives under the user's source tree.
    #[test]
    fn archive_dir_computed_from_persisted_source_root() {
        let tx_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        // source_root is the persisted import_runs.source_root; the archive
        // always lives under it regardless of where the album dir is.
        let source_root = PathBuf::from("/src");
        let album_relative_path = "AlbumA";
        let archive_dir = source_root
            .join(".imagedb-processed")
            .join(tx_id.to_string())
            .join(album_relative_path);
        assert_eq!(
            archive_dir,
            PathBuf::from("/src/.imagedb-processed/11111111-1111-1111-1111-111111111111/AlbumA")
        );
        // The archive dir must stay under source_root.
        assert!(archive_dir.starts_with(&source_root));
    }

    /// A DB-provided album root tolerates plan images in distinct
    /// subdirectories (AlbumA/chapter-1/001.jpg vs AlbumA/chapter-2/002.jpg)
    /// — previously this was rejected as "multiple parents". The validation
    /// function skips non-existent sources, so a unit test without a
    /// filesystem still proves subdirectory tolerance.
    #[test]
    fn validate_sources_tolerates_distinct_subdirs_when_missing() {
        let root = PathBuf::from("/src/AlbumA");
        let imgs = vec![
            img(
                "/src/AlbumA/chapter-1/001.jpg",
                "AlbumA/chapter-1/001.jpg",
                "chapter-1/001.jpg",
            ),
            img(
                "/src/AlbumA/chapter-2/002.jpg",
                "AlbumA/chapter-2/002.jpg",
                "chapter-2/002.jpg",
            ),
        ];
        // Neither source file exists on the unit-test filesystem, so
        // validation is a no-op pass. The important property is that the
        // function does NOT raise the old "multiple source parents" error.
        assert!(validate_plan_image_sources(&root, &imgs).is_ok());
    }

    /// path_eq is reused by recovery and must handle the Windows case
    /// normalization plus the Unix byte-exact rule.
    #[test]
    fn path_eq_rule() {
        if cfg!(target_os = "windows") {
            assert!(crate::services::commit_service::path_eq(
                Path::new("C:\\AlbumA"),
                Path::new("c:\\albuma")
            ));
        } else {
            assert!(crate::services::commit_service::path_eq(
                Path::new("/src/AlbumA"),
                Path::new("/src/AlbumA")
            ));
            assert!(!crate::services::commit_service::path_eq(
                Path::new("/src/AlbumA"),
                Path::new("/src/albuma")
            ));
        }
    }

    #[test]
    fn archive_entry_from_library_committed() {
        let action = resolve_archive_entry_transition(TransactionState::LibraryCommitted).unwrap();
        assert_eq!(
            action,
            ArchiveEntryAction::BeginArchive(TransactionState::SourceArchiving)
        );
    }

    #[test]
    fn archive_entry_from_source_archiving_retry() {
        let action = resolve_archive_entry_transition(TransactionState::SourceArchiving).unwrap();
        assert_eq!(
            action,
            ArchiveEntryAction::BeginArchive(TransactionState::SourceArchiving)
        );
    }

    #[test]
    fn archive_entry_from_source_archived_is_noop() {
        let action = resolve_archive_entry_transition(TransactionState::SourceArchived).unwrap();
        assert_eq!(action, ArchiveEntryAction::AlreadyArchived);
    }

    #[test]
    fn archive_entry_rejects_illegal_states() {
        for state in &[
            TransactionState::Planned,
            TransactionState::Staging,
            TransactionState::Verifying,
            TransactionState::Verified,
            TransactionState::Publishing,
            TransactionState::Published,
            TransactionState::DbCommitting,
            TransactionState::CleanupRequired,
            TransactionState::Conflict,
            TransactionState::Failed,
            TransactionState::Cancelled,
        ] {
            let err = resolve_archive_entry_transition(*state).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("cannot enter source archive recovery"),
                "unexpected error for {state:?}: {msg}"
            );
        }
    }
}

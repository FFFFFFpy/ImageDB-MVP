-- Migration 0006: Workflow and idempotency constraints
--
-- Adds CHECK constraints for all state columns and unique/partial indexes
-- to enforce workflow rules and prevent duplicate operations.

-- ── Import run state CHECK ──
ALTER TABLE import_runs DROP CONSTRAINT IF EXISTS chk_import_run_state;
ALTER TABLE import_runs ADD CONSTRAINT chk_import_run_state
    CHECK (state IN (
        'created', 'scanning', 'fingerprinting', 'detecting_duplicates',
        'analyzing', 'review_required', 'ready_to_commit', 'committing',
        'recovery_required', 'completed', 'cancelled', 'failed'
    ));

-- ── Import plan state CHECK ──
ALTER TABLE import_plans DROP CONSTRAINT IF EXISTS chk_import_plan_state;
ALTER TABLE import_plans ADD CONSTRAINT chk_import_plan_state
    CHECK (state IN ('draft', 'frozen', 'consumed', 'invalidated'));

-- ── File transaction state CHECK ──
ALTER TABLE file_transactions DROP CONSTRAINT IF EXISTS chk_file_transaction_state;
ALTER TABLE file_transactions ADD CONSTRAINT chk_file_transaction_state
    CHECK (state IN (
        'planned', 'staging', 'verifying', 'verified', 'publishing',
        'published', 'db_committing', 'library_committed', 'source_archiving',
        'source_archived', 'cleanup_required', 'conflict', 'failed', 'cancelled'
    ));

-- ── File operation state CHECK ──
ALTER TABLE file_operations DROP CONSTRAINT IF EXISTS chk_file_operation_state;
ALTER TABLE file_operations ADD CONSTRAINT chk_file_operation_state
    CHECK (state IN (
        'planned', 'copying', 'copied', 'verifying', 'verified',
        'published', 'failed', 'cancelled'
    ));

-- ── Duplicate candidate scope CHECK ──
ALTER TABLE duplicate_candidates DROP CONSTRAINT IF EXISTS chk_duplicate_scope;
ALTER TABLE duplicate_candidates ADD CONSTRAINT chk_duplicate_scope
    CHECK (scope IN ('intra_album', 'cross_album', 'library'));

-- ── Review decision CHECK ──
ALTER TABLE review_decisions DROP CONSTRAINT IF EXISTS chk_review_decision;
ALTER TABLE review_decisions ADD CONSTRAINT chk_review_decision
    CHECK (decision IN ('keep_source', 'keep_candidate', 'keep_all', 'skip_album'));

-- ── Duplicate candidate decision CHECK ──
ALTER TABLE duplicate_candidates DROP CONSTRAINT IF EXISTS chk_candidate_decision;
ALTER TABLE duplicate_candidates ADD CONSTRAINT chk_candidate_decision
    CHECK (decision IN ('auto_duplicate', 'review') OR decision IS NULL);

-- ── Duplicate candidate decision_source CHECK ──
ALTER TABLE duplicate_candidates DROP CONSTRAINT IF EXISTS chk_candidate_decision_source;
ALTER TABLE duplicate_candidates ADD CONSTRAINT chk_candidate_decision_source
    CHECK (decision_source IN ('exact_rule', 'perceptual_rule') OR decision_source IS NULL);

-- ── Only one active file transaction per import album ──
CREATE UNIQUE INDEX IF NOT EXISTS idx_file_transactions_unique_active
    ON file_transactions (import_album_id) WHERE state NOT IN ('source_archived', 'failed', 'cancelled');

-- ── Normalized pair unique key for duplicate candidates ──
-- Prevents reverse duplicates and retry re-insertion.
-- Uses LEAST/GREATEST to normalize the pair order.
CREATE UNIQUE INDEX IF NOT EXISTS idx_duplicate_candidates_normalized_pair
    ON duplicate_candidates (
        import_run_id,
        LEAST(source_image_id, COALESCE(candidate_source_image_id, candidate_library_image_id, '00000000-0000-0000-0000-000000000000'::uuid)),
        GREATEST(source_image_id, COALESCE(candidate_source_image_id, candidate_library_image_id, '00000000-0000-0000-0000-000000000000'::uuid)),
        scope,
        match_type
    ) WHERE decision IS NOT NULL;

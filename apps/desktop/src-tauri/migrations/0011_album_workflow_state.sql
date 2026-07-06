-- Migration 0011: album workflow checkpoints and dashboard counters.
--
-- These columns make import_albums the durable progress unit for MVP2. The
-- import run remains the commit container; file transactions and frozen plans
-- are not changed by this migration.

ALTER TABLE import_albums
ADD COLUMN IF NOT EXISTS analysis_started_at TIMESTAMPTZ,
ADD COLUMN IF NOT EXISTS analysis_completed_at TIMESTAMPTZ,
ADD COLUMN IF NOT EXISTS last_error_code TEXT,
ADD COLUMN IF NOT EXISTS last_error_message TEXT,
ADD COLUMN IF NOT EXISTS image_count INTEGER NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS fingerprinted_count INTEGER NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS duplicate_candidate_count INTEGER NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS review_candidate_count INTEGER NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS analysis_attempts INTEGER NOT NULL DEFAULT 0,
ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

ALTER TABLE import_albums DROP CONSTRAINT IF EXISTS chk_import_album_state;
ALTER TABLE import_albums ADD CONSTRAINT chk_import_album_state
    CHECK (state IN (
        'pending',
        'scanning',
        'fingerprinting',
        'analyzing',
        'analyzed',
        'review_required',
        'reviewed',
        'ready_to_commit',
        'committing',
        'completed',
        'failed'
    ));

CREATE UNIQUE INDEX IF NOT EXISTS idx_import_albums_run_source_path
    ON import_albums (import_run_id, source_path);

CREATE INDEX IF NOT EXISTS idx_import_albums_run_state
    ON import_albums (import_run_id, state);

CREATE INDEX IF NOT EXISTS idx_import_albums_updated_at
    ON import_albums (updated_at);

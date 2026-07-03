-- Migration 0008: Source album snapshots
--
-- Captures a complete, immutable snapshot of every ordinary file in a
-- source album directory at scan time. This includes images (even those
-- later excluded by dedup), sidecar files (description.txt, .xmp, etc.),
-- and files in nested subdirectories.
--
-- Source snapshots are independent from the commit plan (import_plans /
-- import_plan_images) which only tracks images selected for the library.

CREATE TABLE IF NOT EXISTS source_album_snapshots (
    id UUID PRIMARY KEY,
    import_run_id UUID NOT NULL REFERENCES import_runs(id) ON DELETE CASCADE,
    import_album_id UUID NOT NULL REFERENCES import_albums(id) ON DELETE CASCADE,
    source_album_path TEXT NOT NULL,
    snapshot_hash BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (import_album_id)
);

CREATE INDEX IF NOT EXISTS idx_source_album_snapshots_run
    ON source_album_snapshots (import_run_id);

CREATE TABLE IF NOT EXISTS source_album_snapshot_files (
    id UUID PRIMARY KEY,
    snapshot_id UUID NOT NULL REFERENCES source_album_snapshots(id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    file_type TEXT NOT NULL,
    file_size BIGINT NOT NULL,
    blake3 BYTEA NOT NULL,
    UNIQUE (snapshot_id, relative_path)
);

CREATE INDEX IF NOT EXISTS idx_source_album_snapshot_files_snapshot
    ON source_album_snapshot_files (snapshot_id);

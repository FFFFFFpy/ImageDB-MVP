-- Migration 0005: Import plans for immutable commit planning
--
-- Adds tables for frozen import plans that capture the exact set of
-- images to commit, their source paths, target paths, and expected
-- file properties. Plans are frozen before any file operations begin
-- and are never modified during commit.

CREATE TABLE IF NOT EXISTS import_plans (
    id UUID PRIMARY KEY,
    import_run_id UUID NOT NULL REFERENCES import_runs(id) ON DELETE CASCADE,
    version INTEGER NOT NULL DEFAULT 1,
    state TEXT NOT NULL DEFAULT 'draft',
    policy_version TEXT NOT NULL,
    library_root_id UUID NOT NULL REFERENCES library_roots(id),
    plan_hash BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    frozen_at TIMESTAMPTZ,
    UNIQUE (import_run_id, version)
);

-- Only one FROZEN plan per import run at a time.
CREATE UNIQUE INDEX IF NOT EXISTS idx_import_plans_unique_frozen
    ON import_plans (import_run_id) WHERE state = 'frozen';

CREATE TABLE IF NOT EXISTS import_plan_albums (
    id UUID PRIMARY KEY,
    plan_id UUID NOT NULL REFERENCES import_plans(id) ON DELETE CASCADE,
    import_album_id UUID NOT NULL REFERENCES import_albums(id),
    target_relative_path TEXT NOT NULL,
    expected_image_count INTEGER NOT NULL,
    album_plan_hash BYTEA,
    UNIQUE (plan_id, import_album_id)
);

CREATE TABLE IF NOT EXISTS import_plan_images (
    id UUID PRIMARY KEY,
    plan_album_id UUID NOT NULL REFERENCES import_plan_albums(id) ON DELETE CASCADE,
    import_image_id UUID NOT NULL REFERENCES import_images(id),
    source_path TEXT NOT NULL,
    source_relative_path TEXT NOT NULL,
    target_relative_path TEXT NOT NULL,
    expected_file_size BIGINT NOT NULL,
    expected_blake3 BYTEA NOT NULL,
    width INTEGER,
    height INTEGER,
    format TEXT,
    UNIQUE (plan_album_id, target_relative_path)
);

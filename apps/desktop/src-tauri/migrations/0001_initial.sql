CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE app_meta (
    key TEXT PRIMARY KEY,
    value JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE library_roots (
    id UUID PRIMARY KEY,
    path TEXT NOT NULL,
    display_name TEXT NOT NULL,
    filesystem_type TEXT,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE import_runs (
    id UUID PRIMARY KEY,
    source_root TEXT NOT NULL,
    library_root_id UUID NOT NULL REFERENCES library_roots(id),
    state TEXT NOT NULL,
    policy_version TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    error_code TEXT,
    error_message TEXT,
    statistics JSONB NOT NULL DEFAULT '{}'::jsonb
);

CREATE TABLE import_albums (
    id UUID PRIMARY KEY,
    import_run_id UUID NOT NULL REFERENCES import_runs(id) ON DELETE CASCADE,
    source_path TEXT NOT NULL,
    source_name TEXT NOT NULL,
    source_snapshot_hash BYTEA,
    target_relative_path TEXT,
    state TEXT NOT NULL,
    decision TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    committed_at TIMESTAMPTZ
);

CREATE TABLE import_images (
    id UUID PRIMARY KEY,
    import_album_id UUID NOT NULL REFERENCES import_albums(id) ON DELETE CASCADE,
    source_path TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    file_size BIGINT NOT NULL,
    modified_at TIMESTAMPTZ,
    width INTEGER,
    height INTEGER,
    format TEXT,
    decode_state TEXT NOT NULL,
    blake3 BYTEA,
    pixel_hash BYTEA,
    gradient_hash BYTEA,
    block_hash BYTEA,
    median_hash BYTEA,
    fingerprint_version TEXT,
    quality_score DOUBLE PRECISION,
    state TEXT NOT NULL
);

CREATE TABLE library_albums (
    id UUID PRIMARY KEY,
    library_root_id UUID NOT NULL REFERENCES library_roots(id),
    display_name TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    manifest_version TEXT NOT NULL,
    manifest_hash BYTEA NOT NULL,
    image_count INTEGER NOT NULL,
    committed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    state TEXT NOT NULL,
    UNIQUE (library_root_id, relative_path)
);

CREATE TABLE library_images (
    id UUID PRIMARY KEY,
    album_id UUID NOT NULL REFERENCES library_albums(id) ON DELETE CASCADE,
    relative_path TEXT NOT NULL,
    file_size BIGINT NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    format TEXT NOT NULL,
    blake3 BYTEA NOT NULL,
    pixel_hash BYTEA,
    gradient_hash BYTEA,
    block_hash BYTEA,
    median_hash BYTEA,
    fingerprint_version TEXT NOT NULL,
    quality_score DOUBLE PRECISION,
    committed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    state TEXT NOT NULL,
    UNIQUE (album_id, relative_path)
);

CREATE TABLE duplicate_candidates (
    id UUID PRIMARY KEY,
    import_run_id UUID NOT NULL REFERENCES import_runs(id) ON DELETE CASCADE,
    source_image_id UUID NOT NULL REFERENCES import_images(id) ON DELETE CASCADE,
    candidate_source_image_id UUID REFERENCES import_images(id) ON DELETE CASCADE,
    candidate_library_image_id UUID REFERENCES library_images(id),
    scope TEXT NOT NULL,
    match_type TEXT NOT NULL,
    blake3_equal BOOLEAN NOT NULL DEFAULT FALSE,
    pixel_hash_equal BOOLEAN NOT NULL DEFAULT FALSE,
    gradient_distance INTEGER,
    block_distance INTEGER,
    median_distance INTEGER,
    transform_type TEXT,
    confidence DOUBLE PRECISION,
    decision TEXT,
    decision_source TEXT,
    rule_version TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (candidate_source_image_id IS NOT NULL AND candidate_library_image_id IS NULL)
        OR
        (candidate_source_image_id IS NULL AND candidate_library_image_id IS NOT NULL)
    )
);

CREATE TABLE review_decisions (
    id UUID PRIMARY KEY,
    candidate_id UUID NOT NULL UNIQUE REFERENCES duplicate_candidates(id) ON DELETE CASCADE,
    decision TEXT NOT NULL,
    selected_image_id UUID,
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE file_transactions (
    id UUID PRIMARY KEY,
    import_run_id UUID NOT NULL REFERENCES import_runs(id),
    import_album_id UUID NOT NULL UNIQUE REFERENCES import_albums(id),
    state TEXT NOT NULL,
    staging_path TEXT,
    target_path TEXT,
    manifest_path TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    last_error TEXT
);

CREATE TABLE file_operations (
    id UUID PRIMARY KEY,
    transaction_id UUID NOT NULL REFERENCES file_transactions(id) ON DELETE CASCADE,
    source_path TEXT NOT NULL,
    staging_path TEXT NOT NULL,
    target_path TEXT NOT NULL,
    expected_size BIGINT NOT NULL,
    expected_blake3 BYTEA NOT NULL,
    actual_blake3 BYTEA,
    state TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE audit_events (
    id BIGSERIAL PRIMARY KEY,
    import_run_id UUID REFERENCES import_runs(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

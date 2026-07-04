-- Migration 0010: Database-authoritative library root leases
--
-- Mounted/shared storage file locks are not trusted as the sole writer guard.
-- This table makes PostgreSQL the authority for write ownership of a library
-- root. A storage-side lock file may still be used later as diagnostics only.

CREATE TABLE IF NOT EXISTS library_root_leases (
    library_root_id UUID PRIMARY KEY REFERENCES library_roots(id) ON DELETE CASCADE,
    owner_instance_id TEXT NOT NULL,
    lease_token UUID NOT NULL UNIQUE,
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (expires_at > heartbeat_at)
);

CREATE INDEX IF NOT EXISTS idx_library_root_leases_expires
    ON library_root_leases (expires_at);

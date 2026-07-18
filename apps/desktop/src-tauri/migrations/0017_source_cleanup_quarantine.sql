-- Migration 0017: bind source cleanup verification to the exact file that is deleted.
--
-- The quarantine path is persisted before any rename. It is nullable only for
-- cleanup rows created by 0016 before this migration; recovery assigns those
-- rows a deterministic unique path before touching the source file.

ALTER TABLE source_file_cleanup_operations
    ADD COLUMN quarantine_path TEXT;

CREATE UNIQUE INDEX idx_source_file_cleanup_quarantine_path
    ON source_file_cleanup_operations (transaction_id, quarantine_path)
    WHERE quarantine_path IS NOT NULL;

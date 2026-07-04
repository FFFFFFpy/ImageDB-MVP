-- Migration 0009: Drop the redundant import_albums.source_snapshot_hash column
--
-- The same snapshot hash is persisted (authoritatively) on
-- source_album_snapshots.snapshot_hash, written by the same
-- insert_source_album_snapshot call that wrote this column. Keeping two
-- copies that are never cross-checked by the commit/recovery main chain
-- is a redundant-evidence hazard: a tampered value on one column cannot be
-- detected by the pipeline.
--
-- The commit and recovery services read source_album_snapshots.snapshot_hash
-- (via load_source_album_snapshot), so dropping import_albums.source_snapshot_hash
-- removes the unused duplicate without changing any verified behavior.

ALTER TABLE import_albums DROP COLUMN IF EXISTS source_snapshot_hash;

-- Migration 0007: Link transactions to plans and library albums to transactions
--
-- The commit pipeline reads the frozen plan as its sole source of truth.
-- To make idempotency verification complete, the file transaction must
-- remember the plan hash and manifest hash it was built from, and the
-- published library album must remember the transaction + plan that
-- produced it.

-- plan hash + manifest hash on file_transactions (recovery evidence).
ALTER TABLE file_transactions ADD COLUMN IF NOT EXISTS plan_hash BYTEA;
ALTER TABLE file_transactions ADD COLUMN IF NOT EXISTS manifest_hash BYTEA;

-- Link library_albums back to the file transaction + frozen plan that
-- produced them. This is what makes idempotent recovery authoritative:
-- a library_album is "already committed" only if its transaction_id,
-- plan_hash and manifest_hash all match the recovered transaction.
ALTER TABLE library_albums ADD COLUMN IF NOT EXISTS transaction_id UUID;
ALTER TABLE library_albums ADD COLUMN IF NOT EXISTS plan_hash BYTEA;

-- Index for locating a library album by its producing transaction.
CREATE INDEX IF NOT EXISTS idx_library_albums_transaction
    ON library_albums (transaction_id) WHERE transaction_id IS NOT NULL;

-- Index for finding the file transaction that produced a library album.
CREATE INDEX IF NOT EXISTS idx_file_transactions_plan
    ON file_transactions (import_run_id) WHERE plan_hash IS NOT NULL;

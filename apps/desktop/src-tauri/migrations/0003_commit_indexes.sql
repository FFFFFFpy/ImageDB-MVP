CREATE INDEX IF NOT EXISTS idx_library_albums_root_path
    ON library_albums (library_root_id, relative_path);

CREATE INDEX IF NOT EXISTS idx_library_images_album
    ON library_images (album_id);

CREATE INDEX IF NOT EXISTS idx_file_transactions_run
    ON file_transactions (import_run_id);

CREATE INDEX IF NOT EXISTS idx_file_transactions_album
    ON file_transactions (import_album_id);

CREATE INDEX IF NOT EXISTS idx_file_operations_transaction
    ON file_operations (transaction_id);

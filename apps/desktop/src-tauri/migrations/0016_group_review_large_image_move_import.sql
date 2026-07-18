-- Migration 0016: persisted group review and selected-file move semantics.
--
-- Existing unfinished runs are intentionally not backfilled into review_groups:
-- edge-level decisions cannot be converted into a group-wide final selection
-- without guessing. The service rejects those runs and asks for re-analysis.

CREATE TABLE review_groups (
    id UUID PRIMARY KEY,
    import_run_id UUID NOT NULL REFERENCES import_runs(id) ON DELETE CASCADE,
    state TEXT NOT NULL,
    requires_manual_review BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    CHECK (state IN ('pending', 'resolved')),
    CHECK ((state = 'resolved') = (resolved_at IS NOT NULL))
);

CREATE INDEX idx_review_groups_run_state
    ON review_groups (import_run_id, state, created_at, id);

CREATE TABLE review_group_members (
    id UUID PRIMARY KEY,
    group_id UUID NOT NULL REFERENCES review_groups(id) ON DELETE CASCADE,
    image_id UUID NOT NULL,
    image_source TEXT NOT NULL,
    final_action TEXT NOT NULL,
    decision_source TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (group_id, image_source, image_id),
    CHECK (image_source IN ('import', 'library')),
    CHECK (final_action IN ('keep', 'exclude')),
    CHECK (decision_source IN ('automatic', 'user')),
    CHECK (image_source <> 'library' OR final_action = 'keep')
);

CREATE INDEX idx_review_group_members_group
    ON review_group_members (group_id, image_source, image_id);

-- A run-level uniqueness constraint prevents one image from being materialized
-- into multiple final review groups. The trigger also verifies that the UUID
-- belongs to the group's run (import) or to the bound library (library).
CREATE OR REPLACE FUNCTION enforce_review_group_member_semantics()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE group_run UUID;
DECLARE group_library_root UUID;
BEGIN
    SELECT rg.import_run_id, ir.library_root_id
      INTO group_run, group_library_root
      FROM review_groups rg
      JOIN import_runs ir ON ir.id = rg.import_run_id
     WHERE rg.id = NEW.group_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'review group % does not exist', NEW.group_id;
    END IF;

    IF NEW.image_source = 'import' THEN
        IF NOT EXISTS (
            SELECT 1 FROM import_images ii
            JOIN import_albums ia ON ia.id = ii.import_album_id
            WHERE ii.id = NEW.image_id AND ia.import_run_id = group_run
        ) THEN
            RAISE EXCEPTION 'import image % does not belong to review group run %',
                NEW.image_id, group_run;
        END IF;
    ELSE
        IF NEW.final_action <> 'keep' THEN
            RAISE EXCEPTION 'library review member % must remain keep', NEW.image_id;
        END IF;
        IF NOT EXISTS (
            SELECT 1 FROM library_images li
            JOIN library_albums la ON la.id = li.album_id
            WHERE li.id = NEW.image_id AND la.library_root_id = group_library_root
        ) THEN
            RAISE EXCEPTION 'library image % does not belong to review group library root %',
                NEW.image_id, group_library_root;
        END IF;
    END IF;

    IF EXISTS (
        SELECT 1 FROM review_group_members other
        JOIN review_groups other_group ON other_group.id = other.group_id
        WHERE other_group.import_run_id = group_run
          AND other.image_source = NEW.image_source
          AND other.image_id = NEW.image_id
          AND other.id <> NEW.id
    ) THEN
        RAISE EXCEPTION 'image % (%) already belongs to another review group for run %',
            NEW.image_id, NEW.image_source, group_run;
    END IF;

    NEW.updated_at = now();
    RETURN NEW;
END $$;

CREATE TRIGGER trg_review_group_member_semantics
BEFORE INSERT OR UPDATE OF group_id, image_id, image_source, final_action
ON review_group_members FOR EACH ROW
EXECUTE FUNCTION enforce_review_group_member_semantics();

ALTER TABLE import_plans
    ADD COLUMN source_file_mode TEXT NOT NULL DEFAULT 'copy_and_archive';
ALTER TABLE import_plans
    ADD CONSTRAINT chk_import_plan_source_file_mode
    CHECK (source_file_mode IN ('copy_and_archive', 'move_selected_without_backup'));

ALTER TABLE file_transactions
    ADD COLUMN source_file_mode TEXT NOT NULL DEFAULT 'copy_and_archive';
ALTER TABLE file_transactions
    ADD CONSTRAINT chk_file_transaction_source_file_mode
    CHECK (source_file_mode IN ('copy_and_archive', 'move_selected_without_backup'));

CREATE TABLE source_file_cleanup_operations (
    id UUID PRIMARY KEY,
    transaction_id UUID NOT NULL REFERENCES file_transactions(id) ON DELETE CASCADE,
    source_path TEXT NOT NULL,
    expected_size BIGINT NOT NULL,
    expected_blake3 BYTEA NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    last_error TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (transaction_id, source_path),
    CHECK (expected_size >= 0),
    CHECK (octet_length(expected_blake3) = 32),
    CHECK (state IN ('pending', 'verifying', 'removing', 'removed', 'conflict'))
);

CREATE INDEX idx_source_file_cleanup_transaction_state
    ON source_file_cleanup_operations (transaction_id, state, source_path);

ALTER TABLE file_transactions DROP CONSTRAINT IF EXISTS chk_file_transaction_state;
ALTER TABLE file_transactions ADD CONSTRAINT chk_file_transaction_state
    CHECK (state IN (
        'planned', 'staging', 'verifying', 'verified', 'publishing',
        'published', 'db_committing', 'library_committed', 'source_archiving',
        'source_archived', 'source_files_removing', 'source_files_removed',
        'cleanup_required', 'conflict', 'failed', 'cancelled'
    ));

DROP INDEX IF EXISTS idx_file_transactions_unique_active;
CREATE UNIQUE INDEX idx_file_transactions_unique_active
    ON file_transactions (import_album_id)
    WHERE state NOT IN ('source_archived', 'source_files_removed', 'failed', 'cancelled');

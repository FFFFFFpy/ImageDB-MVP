-- Migration 0018: persist editable draft inclusion without changing frozen plans.
--
-- Draft plans retain every eligible import image so toggling an image off and
-- back on preserves its target album and target relative path. Freeze removes
-- excluded rows in the same database transaction before computing the existing
-- plan hash, so frozen-plan, Commit, Manifest, and Recovery semantics are
-- unchanged.

ALTER TABLE import_plan_images
    ADD COLUMN included BOOLEAN NOT NULL DEFAULT TRUE;

CREATE INDEX idx_import_plan_images_included
    ON import_plan_images (plan_album_id, included, target_relative_path);

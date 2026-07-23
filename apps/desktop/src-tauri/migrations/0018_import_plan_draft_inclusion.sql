-- Migration 0018: persist editable draft inclusion without changing frozen plans.
--
-- Draft plans retain every eligible import image so toggling an image off and
-- back on preserves its target album and target relative path. Freeze removes
-- excluded rows in the same database transaction before computing the existing
-- plan hash, so frozen-plan, Commit, Manifest, and Recovery semantics are
-- unchanged.

-- Drafts created before this migration only persisted included images. They
-- cannot safely support re-enabling a skipped image because the corresponding
-- plan row does not exist. Invalidate only those editable drafts so the
-- workflow resolver returns the run to generate_plan and creates a complete
-- replacement. Frozen/consumed plans and file transactions are untouched.
UPDATE import_plans
SET state = 'invalidated'
WHERE state = 'draft';

ALTER TABLE import_plan_images
    ADD COLUMN included BOOLEAN NOT NULL DEFAULT TRUE;

CREATE INDEX idx_import_plan_images_included
    ON import_plan_images (plan_album_id, included, target_relative_path);

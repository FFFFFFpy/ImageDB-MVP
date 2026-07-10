-- Migration 0012: repair and normalize album workflow checkpoints.
--
-- Migration 0011 was already published with the legacy album states still
-- accepted.  Never rewrite an applied migration: this follow-up upgrades both
-- databases created by that original migration and databases that briefly ran
-- a locally edited 0011 which normalized in-flight rows without cleaning their
-- partial images.

ALTER TABLE import_albums DROP CONSTRAINT IF EXISTS chk_import_album_state;

-- A pending/in-flight album must not have reached frozen-plan or file-
-- transaction territory.  Refuse an inconsistent database instead of
-- deleting evidence needed by Commit/Recovery.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM import_albums ia
        WHERE ia.state IN ('pending', 'scanning', 'fingerprinting', 'analyzing')
          AND (
              EXISTS (
                  SELECT 1
                  FROM import_plan_albums ipa
                  WHERE ipa.import_album_id = ia.id
              )
              OR EXISTS (
                  SELECT 1
                  FROM file_transactions ft
                  WHERE ft.import_album_id = ia.id
              )
          )
    ) THEN
        RAISE EXCEPTION
            'cannot repair pending/in-flight album rows referenced by an import plan or file transaction';
    END IF;
END
$$;

-- Delete partial candidates first so review_decisions cascade with them, then
-- delete partial images.  Source snapshots are deliberately retained: they
-- are the immutable pre-interruption evidence which resume must re-verify.
DELETE FROM duplicate_candidates dc
WHERE dc.source_image_id IN (
        SELECT ii.id
        FROM import_images ii
        JOIN import_albums ia ON ia.id = ii.import_album_id
        WHERE ia.state IN ('pending', 'scanning', 'fingerprinting', 'analyzing')
    )
   OR dc.candidate_source_image_id IN (
        SELECT ii.id
        FROM import_images ii
        JOIN import_albums ia ON ia.id = ii.import_album_id
        WHERE ia.state IN ('pending', 'scanning', 'fingerprinting', 'analyzing')
    );

DELETE FROM import_images ii
USING import_albums ia
WHERE ii.import_album_id = ia.id
  AND ia.state IN ('pending', 'scanning', 'fingerprinting', 'analyzing');

UPDATE import_albums
SET state = 'pending',
    analysis_started_at = NULL,
    analysis_completed_at = NULL,
    last_error_code = NULL,
    last_error_message = NULL,
    image_count = 0,
    fingerprinted_count = 0,
    duplicate_candidate_count = 0,
    review_candidate_count = 0,
    updated_at = now()
WHERE state IN ('pending', 'scanning', 'fingerprinting', 'analyzing');

-- Normalize legacy terminal album states.  Undecided candidates take
-- precedence so upgraded runs remain reviewable instead of silently looking
-- complete.
UPDATE import_albums ia
SET state = CASE
        WHEN EXISTS (
            SELECT 1
            FROM duplicate_candidates dc
            JOIN import_images si ON si.id = dc.source_image_id
            LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
            WHERE si.import_album_id = ia.id
              AND dc.decision IS NULL
              AND rd.id IS NULL
        ) THEN 'review_required'
        ELSE 'analyzed'
    END,
    analysis_completed_at = COALESCE(analysis_completed_at, committed_at, now()),
    last_error_code = NULL,
    last_error_message = NULL,
    updated_at = now()
WHERE state IN (
    'analyzed',
    'review_required',
    'reviewed',
    'ready_to_commit',
    'committing',
    'completed'
);

-- 0011 added counters with DEFAULT 0.  Backfill historical rows using the
-- same ownership rule as refresh_album_workflow_summary: a candidate belongs
-- to the album containing its source image.
UPDATE import_albums ia
SET image_count = (
        SELECT COUNT(*)::INTEGER
        FROM import_images ii
        WHERE ii.import_album_id = ia.id
    ),
    fingerprinted_count = (
        SELECT COUNT(*)::INTEGER
        FROM import_images ii
        WHERE ii.import_album_id = ia.id
          AND ii.state = 'fingerprinted'
    ),
    duplicate_candidate_count = (
        SELECT COUNT(*)::INTEGER
        FROM duplicate_candidates dc
        JOIN import_images si ON si.id = dc.source_image_id
        WHERE si.import_album_id = ia.id
    ),
    review_candidate_count = (
        SELECT COUNT(*)::INTEGER
        FROM duplicate_candidates dc
        JOIN import_images si ON si.id = dc.source_image_id
        LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
        WHERE si.import_album_id = ia.id
          AND dc.decision IS NULL
          AND rd.id IS NULL
    ),
    updated_at = now();

ALTER TABLE import_albums ADD CONSTRAINT chk_import_album_state
    CHECK (state IN (
        'pending',
        'analyzing',
        'analyzed',
        'review_required',
        'failed'
    ));

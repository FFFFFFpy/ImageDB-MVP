-- Migration 0013: close workflow escape hatches and candidate pair uniqueness.

-- A user may explicitly abandon an analysis checkpoint without deleting its
-- immutable evidence. A later ordinary start creates a separate run.
ALTER TABLE import_runs DROP CONSTRAINT IF EXISTS chk_import_run_state;
ALTER TABLE import_runs ADD CONSTRAINT chk_import_run_state
    CHECK (state IN (
        'created', 'scanning', 'fingerprinting', 'detecting_duplicates',
        'analyzing', 'review_required', 'ready_to_commit', 'committing',
        'recovery_required', 'completed', 'cancelled', 'failed', 'abandoned'
    ));

-- Refuse to guess if duplicate rows already carry contradictory human review
-- decisions. This keeps migration fail-closed for review evidence.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM duplicate_candidates dc
        JOIN review_decisions rd ON rd.candidate_id = dc.id
        WHERE dc.candidate_source_image_id IS NOT NULL
        GROUP BY
            dc.import_run_id,
            LEAST(dc.source_image_id, dc.candidate_source_image_id),
            GREATEST(dc.source_image_id, dc.candidate_source_image_id)
        HAVING COUNT(DISTINCT rd.decision) > 1
    ) OR EXISTS (
        SELECT 1
        FROM duplicate_candidates dc
        JOIN review_decisions rd ON rd.candidate_id = dc.id
        WHERE dc.candidate_library_image_id IS NOT NULL
        GROUP BY dc.import_run_id, dc.source_image_id, dc.candidate_library_image_id
        HAVING COUNT(DISTINCT rd.decision) > 1
    ) THEN
        RAISE EXCEPTION 'duplicate candidate pairs contain conflicting review decisions';
    END IF;
END $$;

-- Prefer an already-reviewed row, then the strongest match type, when old
-- builds produced more than one candidate for the same import/import pair.
WITH ranked AS (
    SELECT dc.id,
           ROW_NUMBER() OVER (
               PARTITION BY dc.import_run_id,
                            LEAST(dc.source_image_id, dc.candidate_source_image_id),
                            GREATEST(dc.source_image_id, dc.candidate_source_image_id)
               ORDER BY (rd.id IS NOT NULL) DESC,
                        CASE dc.match_type
                            WHEN 'file_exact' THEN 0
                            WHEN 'pixel_exact' THEN 1
                            WHEN 'perceptual_near' THEN 2
                            ELSE 3
                        END,
                        dc.created_at,
                        dc.id
           ) AS rn
    FROM duplicate_candidates dc
    LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
    WHERE dc.candidate_source_image_id IS NOT NULL
)
DELETE FROM duplicate_candidates dc
USING ranked r
WHERE dc.id = r.id AND r.rn > 1;

-- Same repair for import/library pairs.
WITH ranked AS (
    SELECT dc.id,
           ROW_NUMBER() OVER (
               PARTITION BY dc.import_run_id, dc.source_image_id, dc.candidate_library_image_id
               ORDER BY (rd.id IS NOT NULL) DESC,
                        CASE dc.match_type
                            WHEN 'file_exact' THEN 0
                            WHEN 'pixel_exact' THEN 1
                            WHEN 'perceptual_near' THEN 2
                            ELSE 3
                        END,
                        dc.created_at,
                        dc.id
           ) AS rn
    FROM duplicate_candidates dc
    LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
    WHERE dc.candidate_library_image_id IS NOT NULL
)
DELETE FROM duplicate_candidates dc
USING ranked r
WHERE dc.id = r.id AND r.rn > 1;

DROP INDEX IF EXISTS idx_duplicate_candidates_normalized_pair;

CREATE UNIQUE INDEX idx_duplicate_candidates_import_pair
    ON duplicate_candidates (
        import_run_id,
        LEAST(source_image_id, candidate_source_image_id),
        GREATEST(source_image_id, candidate_source_image_id)
    )
    WHERE candidate_source_image_id IS NOT NULL;

CREATE UNIQUE INDEX idx_duplicate_candidates_library_pair
    ON duplicate_candidates (import_run_id, source_image_id, candidate_library_image_id)
    WHERE candidate_library_image_id IS NOT NULL;

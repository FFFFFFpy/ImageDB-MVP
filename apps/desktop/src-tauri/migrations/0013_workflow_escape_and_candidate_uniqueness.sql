-- Migration 0013: close workflow escape hatches and candidate pair uniqueness.

ALTER TABLE import_runs DROP CONSTRAINT IF EXISTS chk_import_run_state;
ALTER TABLE import_runs ADD CONSTRAINT chk_import_run_state
    CHECK (state IN (
        'created', 'scanning', 'fingerprinting', 'detecting_duplicates',
        'analyzing', 'review_required', 'ready_to_commit', 'committing',
        'recovery_required', 'completed', 'cancelled', 'failed', 'abandoned'
    ));

-- Validate every human decision before considering duplicate removal. A
-- decision and selected_image_id are one semantic unit; neither is safe to
-- infer from the other when they disagree.
DO $$
DECLARE invalid RECORD;
BEGIN
    SELECT dc.import_run_id, dc.id AS candidate_id, rd.id AS decision_id,
           rd.decision, rd.selected_image_id
      INTO invalid
      FROM duplicate_candidates dc
      JOIN review_decisions rd ON rd.candidate_id = dc.id
     WHERE NOT CASE rd.decision
         WHEN 'keep_source' THEN rd.selected_image_id IS NOT DISTINCT FROM dc.source_image_id
         WHEN 'keep_candidate' THEN rd.selected_image_id IS NOT DISTINCT FROM
             COALESCE(dc.candidate_source_image_id, dc.candidate_library_image_id)
         WHEN 'keep_all' THEN rd.selected_image_id IS NULL
         WHEN 'skip_album' THEN rd.selected_image_id IS NULL
         ELSE FALSE
     END
     LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION
            'invalid review decision structure for run %, candidate %, decision %: decision=%, selected_image_id=%',
            invalid.import_run_id, invalid.candidate_id, invalid.decision_id,
            invalid.decision, invalid.selected_image_id;
    END IF;
END $$;

-- Import/import pairs are directionless. Compare their normalized final
-- outcome, not the direction-relative decision label.
DO $$
DECLARE conflict RECORD;
BEGIN
    SELECT dc.import_run_id,
           LEAST(dc.source_image_id, dc.candidate_source_image_id) AS image_a,
           GREATEST(dc.source_image_id, dc.candidate_source_image_id) AS image_b
      INTO conflict
      FROM duplicate_candidates dc
      JOIN review_decisions rd ON rd.candidate_id = dc.id
     WHERE dc.candidate_source_image_id IS NOT NULL
     GROUP BY dc.import_run_id,
              LEAST(dc.source_image_id, dc.candidate_source_image_id),
              GREATEST(dc.source_image_id, dc.candidate_source_image_id)
    HAVING COUNT(DISTINCT CASE rd.decision
               WHEN 'keep_source' THEN dc.source_image_id::text
               WHEN 'keep_candidate' THEN dc.candidate_source_image_id::text
               WHEN 'keep_all' THEN '__KEEP_ALL__'
               WHEN 'skip_album' THEN '__SKIP_ALBUM__'
               ELSE '__UNKNOWN__:' || rd.decision
           END) > 1
     LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION
            'import/import pair for run %, images %/% has conflicting normalized review outcomes',
            conflict.import_run_id, conflict.image_a, conflict.image_b;
    END IF;
END $$;

DO $$
DECLARE conflict RECORD;
BEGIN
    SELECT dc.import_run_id, dc.source_image_id, dc.candidate_library_image_id
      INTO conflict
      FROM duplicate_candidates dc
      JOIN review_decisions rd ON rd.candidate_id = dc.id
     WHERE dc.candidate_library_image_id IS NOT NULL
     GROUP BY dc.import_run_id, dc.source_image_id, dc.candidate_library_image_id
    HAVING COUNT(DISTINCT CASE rd.decision
               WHEN 'keep_source' THEN dc.source_image_id::text
               WHEN 'keep_candidate' THEN dc.candidate_library_image_id::text
               WHEN 'keep_all' THEN '__KEEP_ALL__'
               WHEN 'skip_album' THEN '__SKIP_ALBUM__'
               ELSE '__UNKNOWN__:' || rd.decision
           END) > 1
     LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION
            'import/library pair for run %, images %/% has conflicting normalized review outcomes',
            conflict.import_run_id, conflict.source_image_id,
            conflict.candidate_library_image_id;
    END IF;
END $$;

-- Choose the survivor only after all validation has succeeded. If equivalent
-- reviewed reverse rows exist, rewrite the survivor's direction-relative
-- fields while preserving the normalized selected image.
WITH candidates AS (
    SELECT dc.*,
           rd.id AS review_id,
           CASE rd.decision
               WHEN 'keep_source' THEN dc.source_image_id::text
               WHEN 'keep_candidate' THEN dc.candidate_source_image_id::text
               WHEN 'keep_all' THEN '__KEEP_ALL__'
               WHEN 'skip_album' THEN '__SKIP_ALBUM__'
           END AS normalized_outcome,
           ROW_NUMBER() OVER (
               PARTITION BY dc.import_run_id,
                            LEAST(dc.source_image_id, dc.candidate_source_image_id),
                            GREATEST(dc.source_image_id, dc.candidate_source_image_id)
               ORDER BY (rd.id IS NOT NULL) DESC,
                        CASE dc.match_type
                            WHEN 'file_exact' THEN 0
                            WHEN 'pixel_exact' THEN 1
                            WHEN 'perceptual_near' THEN 2
                            WHEN 'perceptual_similar' THEN 3
                            ELSE 4
                        END,
                        dc.created_at, dc.id
           ) AS rn
      FROM duplicate_candidates dc
      LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
     WHERE dc.candidate_source_image_id IS NOT NULL
), outcomes AS (
    SELECT import_run_id,
           LEAST(source_image_id, candidate_source_image_id) AS image_a,
           GREATEST(source_image_id, candidate_source_image_id) AS image_b,
           MIN(normalized_outcome) FILTER (WHERE normalized_outcome IS NOT NULL) AS outcome
      FROM candidates
     GROUP BY import_run_id,
              LEAST(source_image_id, candidate_source_image_id),
              GREATEST(source_image_id, candidate_source_image_id)
), survivors AS (
    SELECT c.*, o.outcome
      FROM candidates c
      JOIN outcomes o
        ON o.import_run_id = c.import_run_id
       AND o.image_a = LEAST(c.source_image_id, c.candidate_source_image_id)
       AND o.image_b = GREATEST(c.source_image_id, c.candidate_source_image_id)
     WHERE c.rn = 1 AND c.review_id IS NOT NULL
)
UPDATE review_decisions rd
   SET decision = CASE
           WHEN s.outcome = '__KEEP_ALL__' THEN 'keep_all'
           WHEN s.outcome = '__SKIP_ALBUM__' THEN 'skip_album'
           WHEN s.outcome = s.source_image_id::text THEN 'keep_source'
           WHEN s.outcome = s.candidate_source_image_id::text THEN 'keep_candidate'
       END,
       selected_image_id = CASE
           WHEN s.outcome IN ('__KEEP_ALL__', '__SKIP_ALBUM__') THEN NULL
           ELSE s.outcome::uuid
       END
  FROM survivors s
 WHERE rd.id = s.review_id;

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
                            WHEN 'perceptual_similar' THEN 3
                            ELSE 4
                        END,
                        dc.created_at, dc.id
           ) AS rn
      FROM duplicate_candidates dc
      LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
     WHERE dc.candidate_source_image_id IS NOT NULL
)
DELETE FROM duplicate_candidates dc USING ranked r
 WHERE dc.id = r.id AND r.rn > 1;

WITH ranked AS (
    SELECT dc.id,
           ROW_NUMBER() OVER (
               PARTITION BY dc.import_run_id, dc.source_image_id,
                            dc.candidate_library_image_id
               ORDER BY (rd.id IS NOT NULL) DESC,
                        CASE dc.match_type
                            WHEN 'file_exact' THEN 0
                            WHEN 'pixel_exact' THEN 1
                            WHEN 'perceptual_near' THEN 2
                            WHEN 'perceptual_similar' THEN 3
                            ELSE 4
                        END,
                        dc.created_at, dc.id
           ) AS rn
      FROM duplicate_candidates dc
      LEFT JOIN review_decisions rd ON rd.candidate_id = dc.id
     WHERE dc.candidate_library_image_id IS NOT NULL
)
DELETE FROM duplicate_candidates dc USING ranked r
 WHERE dc.id = r.id AND r.rn > 1;

DROP INDEX IF EXISTS idx_duplicate_candidates_normalized_pair;
CREATE UNIQUE INDEX idx_duplicate_candidates_import_pair
    ON duplicate_candidates (
        import_run_id,
        LEAST(source_image_id, candidate_source_image_id),
        GREATEST(source_image_id, candidate_source_image_id)
    ) WHERE candidate_source_image_id IS NOT NULL;
CREATE UNIQUE INDEX idx_duplicate_candidates_library_pair
    ON duplicate_candidates (import_run_id, source_image_id, candidate_library_image_id)
    WHERE candidate_library_image_id IS NOT NULL;

-- Migration 0014: enforce review decision semantics after the 0013 repair.
-- Databases that ran the earlier development copy of 0013 can only validate
-- evidence that remains; rows already deleted by that copy are unrecoverable.

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
            'invalid remaining review decision structure for run %, candidate %, decision %: decision=%, selected_image_id=%',
            invalid.import_run_id, invalid.candidate_id, invalid.decision_id,
            invalid.decision, invalid.selected_image_id;
    END IF;
END $$;

CREATE OR REPLACE FUNCTION enforce_review_decision_semantics()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE candidate duplicate_candidates%ROWTYPE;
BEGIN
    SELECT * INTO candidate FROM duplicate_candidates WHERE id = NEW.candidate_id;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'review decision references missing candidate %', NEW.candidate_id;
    END IF;
    IF NOT (CASE NEW.decision
        WHEN 'keep_source' THEN NEW.selected_image_id IS NOT DISTINCT FROM candidate.source_image_id
        WHEN 'keep_candidate' THEN NEW.selected_image_id IS NOT DISTINCT FROM
            COALESCE(candidate.candidate_source_image_id, candidate.candidate_library_image_id)
        WHEN 'keep_all' THEN NEW.selected_image_id IS NULL
        WHEN 'skip_album' THEN NEW.selected_image_id IS NULL
        ELSE FALSE
    END) THEN
        RAISE EXCEPTION
            'invalid review decision structure for candidate %: decision=%, selected_image_id=%',
            NEW.candidate_id, NEW.decision, NEW.selected_image_id;
    END IF;
    RETURN NEW;
END $$;

DROP TRIGGER IF EXISTS trg_review_decision_semantics ON review_decisions;
CREATE TRIGGER trg_review_decision_semantics
BEFORE INSERT OR UPDATE OF candidate_id, decision, selected_image_id
ON review_decisions FOR EACH ROW
EXECUTE FUNCTION enforce_review_decision_semantics();

CREATE UNIQUE INDEX IF NOT EXISTS idx_duplicate_candidates_import_pair
    ON duplicate_candidates (
        import_run_id,
        LEAST(source_image_id, candidate_source_image_id),
        GREATEST(source_image_id, candidate_source_image_id)
    ) WHERE candidate_source_image_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_duplicate_candidates_library_pair
    ON duplicate_candidates (import_run_id, source_image_id, candidate_library_image_id)
    WHERE candidate_library_image_id IS NOT NULL;

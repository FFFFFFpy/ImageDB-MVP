-- Migration 0004: Match indexes for perceptual hash bucketing and cross-album detection
--
-- Adds:
-- 1. Perceptual hash band columns for bucketed similarity search
-- 2. Cross-album duplicate scope
-- 3. Batch lookup indexes

-- Add perceptual band columns to library_images for bucketed similarity search.
-- Band 0 = first 4 bytes of gradient_hash, Band 1 = next 4 bytes, etc.
-- This enables indexed recall of approximate candidates instead of full N×M scan.
ALTER TABLE library_images ADD COLUMN IF NOT EXISTS perceptual_band_0 BYTEA;
ALTER TABLE library_images ADD COLUMN IF NOT EXISTS perceptual_band_1 BYTEA;
ALTER TABLE library_images ADD COLUMN IF NOT EXISTS perceptual_band_2 BYTEA;
ALTER TABLE library_images ADD COLUMN IF NOT EXISTS perceptual_band_3 BYTEA;

CREATE INDEX IF NOT EXISTS idx_library_images_band_0
    ON library_images (perceptual_band_0) WHERE perceptual_band_0 IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_library_images_band_1
    ON library_images (perceptual_band_1) WHERE perceptual_band_1 IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_library_images_band_2
    ON library_images (perceptual_band_2) WHERE perceptual_band_2 IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_library_images_band_3
    ON library_images (perceptual_band_3) WHERE perceptual_band_3 IS NOT NULL;

-- Add perceptual band columns to import_images for cross-album detection.
ALTER TABLE import_images ADD COLUMN IF NOT EXISTS perceptual_band_0 BYTEA;
ALTER TABLE import_images ADD COLUMN IF NOT EXISTS perceptual_band_1 BYTEA;
ALTER TABLE import_images ADD COLUMN IF NOT EXISTS perceptual_band_2 BYTEA;
ALTER TABLE import_images ADD COLUMN IF NOT EXISTS perceptual_band_3 BYTEA;

CREATE INDEX IF NOT EXISTS idx_import_images_band_0
    ON import_images (perceptual_band_0) WHERE perceptual_band_0 IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_import_images_band_1
    ON import_images (perceptual_band_1) WHERE perceptual_band_1 IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_import_images_band_2
    ON import_images (perceptual_band_2) WHERE perceptual_band_2 IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_import_images_band_3
    ON import_images (perceptual_band_3) WHERE perceptual_band_3 IS NOT NULL;

-- Add cross-album scope value to duplicate_candidates CHECK constraint.
-- The existing CHECK allows (candidate_source_image_id XOR candidate_library_image_id).
-- Cross-album scope uses candidate_source_image_id (pointing to image in another album).
-- No schema change needed for the constraint since it already allows this.

-- Composite index for cross-album duplicate detection queries.
CREATE INDEX IF NOT EXISTS idx_import_images_album_blake3
    ON import_images (import_album_id, blake3) WHERE blake3 IS NOT NULL;

-- Index for efficient "find all images in a run" queries.
CREATE INDEX IF NOT EXISTS idx_import_images_run_album
    ON import_images (import_album_id);

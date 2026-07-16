-- Migration 0015: fixed Fingerprint V2 schema.
--
-- The project has no production fingerprint corpus to preserve. V1 hashes
-- remain identifiable by fingerprint_version, but their algorithm-specific
-- columns and prefix buckets are intentionally removed.

DROP INDEX IF EXISTS idx_library_images_band_0;
DROP INDEX IF EXISTS idx_library_images_band_1;
DROP INDEX IF EXISTS idx_library_images_band_2;
DROP INDEX IF EXISTS idx_library_images_band_3;
DROP INDEX IF EXISTS idx_import_images_band_0;
DROP INDEX IF EXISTS idx_import_images_band_1;
DROP INDEX IF EXISTS idx_import_images_band_2;
DROP INDEX IF EXISTS idx_import_images_band_3;

ALTER TABLE import_images
    ADD COLUMN block_hash_16 BYTEA,
    ADD COLUMN double_gradient_hash_32 BYTEA,
    ADD COLUMN perceptual_eligible BOOLEAN NOT NULL DEFAULT FALSE,
    DROP COLUMN gradient_hash,
    DROP COLUMN block_hash,
    DROP COLUMN median_hash,
    DROP COLUMN perceptual_band_0,
    DROP COLUMN perceptual_band_1,
    DROP COLUMN perceptual_band_2,
    DROP COLUMN perceptual_band_3;

ALTER TABLE library_images
    ADD COLUMN block_hash_16 BYTEA,
    ADD COLUMN double_gradient_hash_32 BYTEA,
    ADD COLUMN perceptual_eligible BOOLEAN NOT NULL DEFAULT FALSE,
    DROP COLUMN gradient_hash,
    DROP COLUMN block_hash,
    DROP COLUMN median_hash,
    DROP COLUMN perceptual_band_0,
    DROP COLUMN perceptual_band_1,
    DROP COLUMN perceptual_band_2,
    DROP COLUMN perceptual_band_3;

ALTER TABLE duplicate_candidates
    ADD COLUMN double_gradient_distance INTEGER,
    ADD COLUMN block_distance_ratio DOUBLE PRECISION,
    ADD COLUMN double_gradient_distance_ratio DOUBLE PRECISION,
    DROP COLUMN gradient_distance,
    DROP COLUMN median_distance;

ALTER TABLE import_images
    ADD CONSTRAINT chk_import_images_fingerprint_v2_lengths CHECK (
        fingerprint_version IS DISTINCT FROM '2'
        OR (
            blake3 IS NOT NULL
            AND pixel_hash IS NOT NULL
            AND block_hash_16 IS NOT NULL
            AND double_gradient_hash_32 IS NOT NULL
            AND octet_length(blake3) = 32
            AND octet_length(pixel_hash) = 32
            AND octet_length(block_hash_16) = 32
            AND octet_length(double_gradient_hash_32) = 68
        )
    );

ALTER TABLE library_images
    ADD CONSTRAINT chk_library_images_fingerprint_v2_lengths CHECK (
        fingerprint_version IS DISTINCT FROM '2'
        OR (
            blake3 IS NOT NULL
            AND pixel_hash IS NOT NULL
            AND block_hash_16 IS NOT NULL
            AND double_gradient_hash_32 IS NOT NULL
            AND octet_length(blake3) = 32
            AND octet_length(pixel_hash) = 32
            AND octet_length(block_hash_16) = 32
            AND octet_length(double_gradient_hash_32) = 68
        )
    );

ALTER TABLE duplicate_candidates
    ADD CONSTRAINT chk_duplicate_candidates_block_ratio CHECK (
        block_distance_ratio IS NULL
        OR block_distance_ratio BETWEEN 0.0 AND 1.0
    ),
    ADD CONSTRAINT chk_duplicate_candidates_double_gradient_ratio CHECK (
        double_gradient_distance_ratio IS NULL
        OR double_gradient_distance_ratio BETWEEN 0.0 AND 1.0
    ),
    ADD CONSTRAINT chk_duplicate_candidates_confidence CHECK (
        confidence IS NULL OR confidence BETWEEN 0.0 AND 1.0
    );

DROP INDEX IF EXISTS idx_import_images_blake3;
DROP INDEX IF EXISTS idx_import_images_pixel_hash;
DROP INDEX IF EXISTS idx_library_images_blake3;
DROP INDEX IF EXISTS idx_library_images_pixel_hash;

CREATE INDEX idx_import_images_blake3_v2
    ON import_images (blake3)
    WHERE fingerprint_version = '2' AND blake3 IS NOT NULL;
CREATE INDEX idx_import_images_pixel_hash_v2
    ON import_images (pixel_hash)
    WHERE fingerprint_version = '2' AND pixel_hash IS NOT NULL;
CREATE INDEX idx_library_images_blake3_v2
    ON library_images (blake3)
    WHERE fingerprint_version = '2';
CREATE INDEX idx_library_images_pixel_hash_v2
    ON library_images (pixel_hash)
    WHERE fingerprint_version = '2' AND pixel_hash IS NOT NULL;

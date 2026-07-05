# Milestone 3 Report: Perceptual Similarity Detection

## Summary

Milestone 3 extends the scan pipeline with perceptual similarity detection.
Gradient Hash, Block Hash, and Median Hash are now persisted for every scanned
image. Rotation and mirror transform variants enable orientation-invariant
comparison. Hamming-distance based candidate generation produces perceptual
match candidates under three configurable strategies (strict, balanced, loose).
Auto-decisions are issued only when evidence is clear; ambiguous candidates are
persisted with no auto-decision for later human review.

## Implementation

### Domain types (`domain/import_state.rs`)

- `MatchType`: added `PerceptualNear` and `PerceptualSimilar` variants.
- `TransformType`: 8 geometric transforms (identity, rot90/180/270, flip_h/v,
  transpose, transverse) with `ALL` constant and Display/from_str round-trip.
- `Decision`: `AutoDuplicate` for clear automatic decisions. Review-needed
  perceptual candidates are persisted with `decision = NULL`.
- `DecisionSource`: `ExactRule` | `PerceptualRule`.
- `MatchingStrategy`: `Strict` | `Balanced` | `Loose` with deterministic
  `perceptual_thresholds()` returning `PerceptualThresholds`.
- `PerceptualThresholds`: `near_max_distance`, `similar_max_total`,
  `auto_decide`.
- `SCAN_POLICY_VERSION` bumped from "1.0" to "2.0".

### Perceptual hashing (`infrastructure/image_fingerprint.rs`)

- `PerceptualHashes` struct: gradient, block, median as hex strings.
- `TransformVariant` struct: transform type + perceptual hashes.
- `fingerprint_image_with_transforms()`: computes canonical fingerprint plus
  all 8 transform variant hashes.
- `compute_transform_variants()`: generates 8 oriented hash sets from an 8x8
  grayscale image.
- `compute_perceptual_hashes_8x8()`: computes gradient, block (upscaled to
  64x64), and median hashes from an 8x8 image.
- `hash_hamming_distance()`: XOR-based hamming distance between hex-encoded
  hashes.
- `transform_gray_8x8()`: applies geometric transforms to the 8x8 grid.
- Algorithm parameters are fixed: HASH_SIZE=8, FINGERPRINT_VERSION=1.

### Repository (`repositories/import_repository.rs`)

- `NewImportImage`: added `gradient_hash`, `block_hash`, `median_hash`
  (Option<Vec<u8>>) fields.
- `NewDuplicateCandidate`: added `gradient_distance`, `block_distance`,
  `median_distance` (Option<i32>), `transform_type` (Option<String>),
  `confidence` (Option<f64>), `decision` (Option<Decision>),
  `decision_source` (Option<DecisionSource>).
- `LibraryImageRow`: added `gradient_hash`, `block_hash`, `median_hash`
  (Option<Vec<u8>>) for perceptual comparison.
- `insert_import_image`: SQL updated to persist perceptual hash BYTEA columns.
- `insert_duplicate_candidate`: SQL updated to persist full evidence including
  perceptual distances, transform, confidence, decision, and rule_version.
- `get_library_images_for_comparison`: SQL updated to return perceptual hashes.

### Scan service (`services/scan_service.rs`)

- `FingerprintedData`: extended with perceptual hash bytes/hex and
  `transform_variants: Vec<TransformVariant>`.
- `fingerprint_image_sync`: uses `fingerprint_image_with_transforms` to compute
  all perceptual data in one pass.
- `PerceptualEvidence`: gradient/block/median distances, best transform, and
  confidence score.
- `compare_perceptual_intra`: pairwise transform-aware comparison for
  intra-album images (checks all 64 transform pairs, picks minimum total
  distance).
- `compare_perceptual_library`: compares import image's 8 variants against
  library image's canonical hashes.
- `classify_perceptual`: maps evidence to (MatchType, Decision, DecisionSource)
  based on strategy thresholds.
- `compose_transform`: matrix-based composition of two transforms.
- Exact matching preserved: FileExact and PixelExact behavior from Milestone 2
  is unchanged, with added decision=evidence fields.
- Strategy defaults to `MatchingStrategy::Balanced`.

### Strategy thresholds

| Strategy | near_max | similar_total | Auto-decide near |
| -------- | -------- | ------------- | ---------------- |
| Strict   | 4 bits   | 12 bits       | Yes              |
| Balanced | 8 bits   | 24 bits       | Yes              |
| Loose    | 12 bits  | 40 bits       | No (review)      |

Each hash has 64 bits max (gradient=56, block=64, median=64). The
`near_max_distance` is the per-hash maximum; `similar_max_total` is the sum
threshold across all three hashes.

## Tests

### image_fingerprint.rs (new tests: 9)

- `test_hamming_distance_identical` - identical hashes yield distance 0.
- `test_hamming_distance_single_bit` - single bit flip yields distance 1.
- `test_hamming_distance_all_different` - 0000 vs ffff yields 16.
- `test_hamming_distance_symmetric` - d(a,b) == d(b,a).
- `test_transform_variants_count` - produces exactly 8 variants.
- `test_transform_identity_matches_canonical` - identity variant hashes match
  canonical hashes.
- `test_transform_rot180_double_rot90` - applying rot90 twice equals rot180.
- `test_perceptual_hashes_deterministic` - same image produces same hashes.
- `test_fingerprint_with_transforms` - full pipeline with variants.
- `test_scaled_image_perceptual_similarity` - 64x64 vs 128x128 has low
  perceptual distance.
- `test_mirrored_image_recallable_via_transforms` - flipped image is found via
  transform-aware comparison.

### import_state.rs (new tests: 5)

- `match_type_round_trip` - all 4 match types serialize/deserialize correctly.
- `transform_type_round_trip` - all 8 transforms serialize/deserialize.
- `decision_display` - auto_duplicate, review strings correct.
- `decision_source_display` - exact_rule, perceptual_rule strings correct.
- `matching_strategy_thresholds` - strict < balanced < loose ordering verified.

### scan_service.rs (new tests: 7)

- `test_strategy_determinism` - same strategy produces same thresholds.
- `test_classify_perceptual_near_auto` - strict + low distance =
  PerceptualNear + AutoDuplicate.
- `test_classify_perceptual_loose_review` - loose + any distance leaves
  decision/source empty for later review.
- `test_classify_perceptual_similar` - high per-hash distance =
  PerceptualSimilar.
- `test_compare_perceptual_intra_identical` - same image yields all-zero
  distances.
- `test_compare_perceptual_intra_different` - checker vs split images yield no
  match under strict.
- `test_compose_transform_identity` - composing with identity returns original.
- `test_compose_transform_inverse` - flip_h+flip_h = identity,
  rot180+rot180 = identity.

### Existing tests updated

- `test_fingerprint_image_sync` - verifies new perceptual hash fields and
  transform variants.
- `real_scan_persists_exact_duplicates` (real-db-tests) - updated with new
  struct fields including perceptual evidence.

## Modified files

- `apps/desktop/src-tauri/src/domain/import_state.rs`
- `apps/desktop/src-tauri/src/infrastructure/image_fingerprint.rs`
- `apps/desktop/src-tauri/src/repositories/import_repository.rs`
- `apps/desktop/src-tauri/src/services/scan_service.rs`

## Execution commands

| Command                              | Result                                                       |
| ------------------------------------ | ------------------------------------------------------------ |
| `pnpm install`                       | PASS - already up to date                                    |
| `pnpm typecheck`                     | PASS                                                         |
| `pnpm test:unit`                     | PASS - 4 frontend tests                                      |
| `pnpm rust:test`                     | PASS - 76 Rust tests, 1 ignored PostgreSQL lifecycle test    |
| `pnpm rust:clippy`                   | PASS - `-D warnings`                                         |
| `pnpm build`                         | PASS - release executable built                              |
| real PostgreSQL lifecycle test       | PASS - managed init, pgvector, migrations, shutdown, restart |
| real scan duplicate persistence test | PASS - `--features real-db-tests`, PostgreSQL + filesystem   |
| release executable smoke launch      | PASS - stayed running for 5 seconds                          |

## Known limitations

1. **Cross-album detection**: Perceptual matching is intra-album only within a
   single import run. Cross-album comparison within the same batch is not
   implemented (same limitation as Milestone 2).
2. **Library comparison is still empty**: No library_images exist yet since
   Milestone 5 (file import) is not implemented. The perceptual library
   comparison code is in place but untested with real data.
3. **Sequential fingerprinting**: No worker pool for parallel fingerprinting
   (same limitation as Milestone 2).
4. **No EXIF orientation**: `apply_orientation` remains a no-op; EXIF rotation
   tags are not read. The transform-variant approach partially compensates for
   this by checking all 8 orientations.
5. **Gradient hash produces 56 bits**: The 8x8 gradient hash generates 56 bits
   (7 bits per row x 8 rows), not 64. Hamming distance handles this
   correctly via length-aware comparison.
6. **Strategy is hardcoded to Balanced**: The scan service currently uses
   `MatchingStrategy::Balanced` as default. Configuration via settings is
   deferred to a future milestone.

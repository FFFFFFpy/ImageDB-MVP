# Milestone 4 Report: Human Review GUI

## Summary

Milestone 4 implements the human review interface for uncertain duplicate
candidates. Users can view side-by-side or overlay comparisons of candidate
image pairs, inspect fingerprint distances and transform relations, and issue
decisions (keep source, keep candidate, keep all, skip album). Decisions are
persisted immediately to PostgreSQL and survive page changes and app restarts.
An import plan is generated after all candidates are reviewed, containing only
the final kept files.

## Implementation

### Domain types (`domain/import_state.rs`)

- `ReviewDecisionAction`: `KeepSource` | `KeepCandidate` | `KeepAll` |
  `SkipAlbum` with Display/from_str round-trip.
- `REVIEW_DECISION_VALUES`: centralized string constant.
- `ReviewCandidateSummary`: queue entry with candidate IDs, scope, match type,
  transform, confidence, album name, and decision status.
- `ReviewCandidateDetail`: full detail with image paths, dimensions, file
  sizes, all fingerprint distances, and existing decision.
- `ReviewProgress`: total/decided/remaining counts with `all_decided` flag.
- `ImportPlanImage`: kept image entry with source path, relative path, size,
  album name.
- `ImportPlan`: complete plan with album count, image count, kept images,
  excluded count, and skipped albums.

### Repository (`repositories/import_repository.rs`)

New row structs:
- `ReviewCandidateRow`, `ReviewCandidateDetailRow`, `ReviewProgressRow`,
  `ImportPlanCandidateRow`, `ImportPlanImageRow`, `AlbumRow`.

New methods on `ImportRepository`:
- `get_review_candidates`: query manual-review candidates where
  `duplicate_candidates.decision IS NULL`, with album and persisted-decision
  joins.
- `get_review_candidate_detail`: full detail with image paths and library-root
  join for historical-library previews.
- `get_review_decision`: check existing decision for a candidate.
- `insert_review_decision_once`: idempotent INSERT for the same decision and
  selected image, with conflicting second decisions rejected.
- `get_review_progress`: count total review candidates and decided count.
- `get_all_candidates_for_import_plan`: all candidates with decisions for plan
  generation.
- `get_all_import_images_with_album`: all import images with album info.
- `get_albums_for_run`: albums for a given import run.
- `get_latest_completed_run`: find the most recent completed import run.

### Review service (`services/review_service.rs`)

- `get_review_queue`: fetch and map review candidate summaries.
- `get_review_detail`: fetch full candidate detail.
- `submit_decision`: validate action, compute selected_image_id, persist.
- `skip_album_candidates`: mark all undecided candidates in an album.
- `get_review_progress`: compute progress statistics.
- `generate_import_plan`: database-driven import plan generation, rejected
  while any review candidates remain undecided.
- `build_import_plan`: pure function implementing the exclusion logic:
  - `auto_duplicate`: source_image_id excluded.
  - manual `keep_source`: intra-album candidate excluded.
  - manual `keep_candidate`: incoming source excluded for intra-album and
    library-scope candidates.
  - manual `keep_all`: both incoming files kept.
  - manual `skip_album`: all incoming images in the album excluded.
  - library images are never part of the import plan because they already
    exist in the historical library.
- `load_image_preview`: reads image file, encodes as base64 data URL.

### Commands (`commands/review.rs`)

8 new Tauri commands:
- `get_review_queue`: returns review candidate queue for an import run.
- `get_review_candidate_detail`: returns full detail for one candidate.
- `submit_review_decision`: persists a review decision.
- `skip_review_album`: marks all undecided candidates in album as skipped.
- `get_review_progress`: returns review progress statistics.
- `generate_import_plan`: generates import plan after review completion.
- `get_latest_completed_import_run`: finds latest completed import run ID.
- `get_image_preview`: returns base64 data URL for an image file.

### Frontend

#### Types (`lib/ipc/types.ts`)

- `ReviewCandidateSummary`, `ReviewCandidateDetail`, `ReviewProgress`,
  `ImportPlanImage`, `ImportPlan`, `ImagePreview`, `ReviewDecision`.

#### API (`lib/ipc/api.ts`)

- `getReviewQueue`, `getReviewCandidateDetail`, `submitReviewDecision`,
  `skipReviewAlbum`, `getReviewProgress`, `generateImportPlan`,
  `getLatestCompletedImportRun`, `getImagePreview`.

#### Router (`hooks/use-router.ts`)

- Added `'review'` route.

#### Navigation (`components/Layout.tsx`)

- Added "审核" nav item.

#### ReviewPage (`pages/ReviewPage.tsx`)

Features:
- Loads latest completed import run on mount.
- Fetches review queue and progress via TanStack Query.
- Filters undecided candidates and navigates through them sequentially.
- Loads image previews via `get_image_preview` command.
- Side-by-side image display with synchronized zoom and pan.
- Overlay comparison mode with adjustable opacity.
- Image info cards: dimensions, file size, path for both images.
- Match detail card: album, scope, match type, transform, all distances.
- Decision buttons: Keep Source [1], Keep Candidate [2], Keep All [3],
  Skip Album [4].
- Keyboard shortcuts: 1-4 for decisions, arrows for navigation, O for
  overlay, R for reset view.
- Mouse wheel zoom and drag-to-pan with native event listener.
- Import plan generation after all candidates are reviewed.
- Import plan view with statistics and kept image table.
- Resume support: state is server-side, page refresh restores progress.

#### Styles (`styles/global.css`)

- Review page layout, image panels, overlay mode.
- Info grid, decision buttons, keyboard hint.
- Import plan summary, statistics cards, kept images table.

## Tests

### import_state.rs (new: 1)

- `review_decision_action_round_trip` - all 4 actions serialize/deserialize.

### review_service.rs (new: 11 unit + 1 ignored real-db)

- `review_decision_action_display_parse` - round-trip for all actions.
- `review_decision_rejects_unknown` - unknown strings rejected.
- `plan_excludes_auto_duplicates` - auto-duplicate source excluded.
- `plan_keep_source_excludes_candidate_intra_album` - candidate excluded.
- `plan_keep_candidate_excludes_source_intra_album` - source excluded.
- `plan_keep_all_keeps_both` - both images kept.
- `plan_skip_album_excludes_all_images_in_album` - entire album excluded.
- `plan_library_scope_keep_source_does_not_exclude_library` - library images
  not excluded in library scope.
- `plan_library_scope_keep_candidate_excludes_source` - choosing the existing
  library match excludes the incoming source image.
- `plan_undecided_review_candidate_not_excluded` - undecided candidates don't
  cause exclusions.
- `plan_empty_run` - empty input produces empty plan.
- `real_review_decision_persists_and_filters_plan` - ignored PostgreSQL
  integration test proving queue resume, immediate decision persistence,
  conflict rejection, and import-plan filtering.

## Modified files

- `apps/desktop/src-tauri/Cargo.toml` (added base64 dependency)
- `apps/desktop/src-tauri/src/domain/import_state.rs`
- `apps/desktop/src-tauri/src/repositories/import_repository.rs`
- `apps/desktop/src-tauri/src/services/mod.rs`
- `apps/desktop/src-tauri/src/services/review_service.rs` (new)
- `apps/desktop/src-tauri/src/commands/mod.rs`
- `apps/desktop/src-tauri/src/commands/review.rs` (new)
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/desktop/src/hooks/use-router.ts`
- `apps/desktop/src/components/Layout.tsx`
- `apps/desktop/src/app/App.tsx`
- `apps/desktop/src/lib/ipc/types.ts`
- `apps/desktop/src/lib/ipc/api.ts`
- `apps/desktop/src/pages/ReviewPage.tsx` (new)
- `apps/desktop/src/styles/global.css`

## Execution commands

| Command | Purpose |
| --- | --- |
| `pnpm typecheck` | TypeScript type checking |
| `pnpm test:unit` | Frontend unit tests |
| `pnpm rust:test` | Rust unit tests |
| `pnpm rust:clippy` | Rust linter |
| `pnpm build` | Full Tauri build |
| `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml real_pgvector_full_lifecycle -- --ignored --nocapture --test-threads=1` | Real PostgreSQL + pgvector lifecycle |
| `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests real_review_decision_persists_and_filters_plan -- --ignored --nocapture --test-threads=1` | Real PostgreSQL review persistence and plan filtering |
| `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --features real-db-tests real_scan_persists_exact_duplicates -- --ignored --nocapture --test-threads=1` | Real scan/candidate regression |
| Release executable smoke check | Started built exe, verified it stayed alive, then stopped it |

## Known limitations

1. **No migration needed**: The existing `review_decisions` table and
   `duplicate_candidates.decision` column from Milestone 1 are sufficient.
2. **Import plan is generated on demand**: Review decisions are persisted and
   the import plan is recomputed from them. No separate import-plan table is
   introduced in this milestone.
3. **Image preview via data URL**: Base64-encoded data URLs are used for image
   preview. Large images (>10MB) may cause memory pressure. A Tauri asset
   protocol approach would be more efficient for very large images.
4. **No review queue pagination**: All candidates are loaded at once. For very
   large import runs (1000+ candidates), pagination would improve performance.
5. **Single import run**: The review page loads the latest completed import
   run. Multi-run selection is deferred to a future milestone.
6. **Skip album is per-candidate**: Skipping an album iterates over all
   undecided candidates in that album rather than using a single SQL update.
   Adequate for typical album sizes but could be optimized.

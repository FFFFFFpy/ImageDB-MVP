use crate::domain::duplicate_group::{build_duplicate_groups, compute_excluded_ids, DuplicateEdge};
use crate::domain::import_state::{
    ImportPlan, ImportPlanImage, ReviewCandidateDetail, ReviewCandidateSummary,
    ReviewDecisionAction, ReviewProgress,
};
use crate::error::AppError;
use crate::repositories::import_repository::{
    AlbumRow, ImportPlanCandidateRow, ImportPlanImageRow, ImportRepository,
};
use base64::Engine;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tokio_postgres::Client;
use uuid::Uuid;

pub async fn get_review_queue(
    client: &Client,
    import_run_id: Uuid,
) -> Result<Vec<ReviewCandidateSummary>, AppError> {
    let rows = ImportRepository::get_review_candidates(client, import_run_id).await?;

    Ok(rows
        .into_iter()
        .map(|r| ReviewCandidateSummary {
            candidate_id: r.candidate_id.to_string(),
            source_image_id: r.source_image_id.to_string(),
            candidate_source_image_id: r.candidate_source_image_id.map(|id| id.to_string()),
            candidate_library_image_id: r.candidate_library_image_id.map(|id| id.to_string()),
            scope: r.scope,
            match_type: r.match_type,
            transform_type: r.transform_type,
            confidence: r.confidence,
            album_name: r.album_name,
            has_decision: r.has_decision,
        })
        .collect())
}

pub async fn get_review_detail(
    client: &Client,
    candidate_id: Uuid,
) -> Result<ReviewCandidateDetail, AppError> {
    let row = ImportRepository::get_review_candidate_detail(client, candidate_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("candidate {candidate_id} not found")))?;

    Ok(ReviewCandidateDetail {
        candidate_id: row.candidate_id.to_string(),
        source_image_id: row.source_image_id.to_string(),
        source_image_path: row.source_image_path,
        source_image_file_size: row.source_image_file_size,
        source_image_width: row.source_image_width,
        source_image_height: row.source_image_height,
        candidate_source_image_id: row.candidate_source_image_id.map(|id| id.to_string()),
        candidate_source_image_path: row.candidate_source_image_path,
        candidate_source_image_file_size: row.candidate_source_image_file_size,
        candidate_source_image_width: row.candidate_source_image_width,
        candidate_source_image_height: row.candidate_source_image_height,
        candidate_library_image_id: row.candidate_library_image_id.map(|id| id.to_string()),
        candidate_library_image_path: row.candidate_library_image_path,
        candidate_library_image_file_size: row.candidate_library_image_file_size,
        candidate_library_image_width: row.candidate_library_image_width,
        candidate_library_image_height: row.candidate_library_image_height,
        scope: row.scope,
        match_type: row.match_type,
        blake3_equal: row.blake3_equal,
        pixel_hash_equal: row.pixel_hash_equal,
        gradient_distance: row.gradient_distance,
        block_distance: row.block_distance,
        median_distance: row.median_distance,
        transform_type: row.transform_type,
        confidence: row.confidence,
        album_name: row.album_name,
        album_id: row.album_id.to_string(),
        existing_decision: row.existing_decision,
    })
}

pub async fn submit_decision(
    client: &Client,
    candidate_id: Uuid,
    action: ReviewDecisionAction,
) -> Result<(), AppError> {
    let row = ImportRepository::get_review_candidate_detail(client, candidate_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("candidate {candidate_id} not found")))?;

    let selected_image_id = match action {
        ReviewDecisionAction::KeepSource => Some(row.source_image_id),
        ReviewDecisionAction::KeepCandidate => row
            .candidate_source_image_id
            .or(row.candidate_library_image_id),
        ReviewDecisionAction::KeepAll | ReviewDecisionAction::SkipAlbum => None,
    };

    let decision_str = action.to_string();
    ImportRepository::insert_review_decision_once(
        client,
        candidate_id,
        &decision_str,
        selected_image_id,
        None,
    )
    .await
}

pub async fn skip_album_candidates(
    client: &Client,
    import_run_id: Uuid,
    album_id: Uuid,
) -> Result<u32, AppError> {
    let candidates = ImportRepository::get_review_candidates(client, import_run_id).await?;
    let mut count = 0u32;

    for c in &candidates {
        if c.has_decision {
            continue;
        }
        let detail = ImportRepository::get_review_candidate_detail(client, c.candidate_id).await?;
        if let Some(d) = detail {
            if d.album_id == album_id {
                ImportRepository::insert_review_decision_once(
                    client,
                    c.candidate_id,
                    &ReviewDecisionAction::SkipAlbum.to_string(),
                    None,
                    Some("album skipped"),
                )
                .await?;
                count += 1;
            }
        }
    }

    Ok(count)
}

pub async fn get_review_progress(
    client: &Client,
    import_run_id: Uuid,
) -> Result<ReviewProgress, AppError> {
    let row = ImportRepository::get_review_progress(client, import_run_id).await?;
    let remaining = row.total.saturating_sub(row.decided);

    Ok(ReviewProgress {
        import_run_id: import_run_id.to_string(),
        total_review_candidates: row.total,
        decided_count: row.decided,
        remaining_count: remaining,
        all_decided: remaining == 0,
    })
}

pub async fn generate_import_plan(
    client: &Client,
    import_run_id: Uuid,
) -> Result<ImportPlan, AppError> {
    let progress = ImportRepository::get_review_progress(client, import_run_id).await?;
    let remaining = progress.total.saturating_sub(progress.decided);
    if remaining > 0 {
        return Err(AppError::Internal(format!(
            "cannot generate import plan while {remaining} review candidates remain undecided"
        )));
    }

    let all_images =
        ImportRepository::get_all_import_images_with_album(client, import_run_id).await?;
    let all_candidates =
        ImportRepository::get_all_candidates_for_import_plan(client, import_run_id).await?;
    let albums = ImportRepository::get_albums_for_run(client, import_run_id).await?;

    let plan = build_import_plan(
        import_run_id.to_string(),
        &all_images,
        &all_candidates,
        &albums,
    );

    Ok(plan)
}

pub fn build_import_plan(
    import_run_id: String,
    all_images: &[ImportPlanImageRow],
    all_candidates: &[ImportPlanCandidateRow],
    albums: &[AlbumRow],
) -> ImportPlan {
    let mut excluded_image_ids: HashSet<Uuid> = HashSet::new();
    let mut skipped_album_ids: HashSet<Uuid> = HashSet::new();

    let album_name_map: HashMap<Uuid, String> = albums
        .iter()
        .map(|a: &AlbumRow| (a.id, a.source_name.clone()))
        .collect();

    // Phase 1: Build duplicate groups from auto-duplicate candidates.
    let auto_edges: Vec<DuplicateEdge> = all_candidates
        .iter()
        .filter(|c| c.candidate_decision.as_deref() == Some("auto_duplicate"))
        .filter_map(|c| {
            let candidate_id = c
                .candidate_source_image_id
                .or(c.candidate_library_image_id)?;
            Some(DuplicateEdge {
                image_a: c.source_image_id,
                image_b: candidate_id,
                a_is_import: true,
                b_is_import: c.candidate_library_image_id.is_none(),
                confidence: c.confidence.unwrap_or(0.5),
                blake3_equal: c.blake3_equal,
                pixel_hash_equal: c.pixel_hash_equal,
            })
        })
        .collect();

    let groups = build_duplicate_groups(&auto_edges);
    let auto_excluded = compute_excluded_ids(&groups);
    excluded_image_ids.extend(auto_excluded);

    // Phase 2: Apply review decisions.
    for c in all_candidates {
        if c.candidate_decision.is_some() {
            // Already handled by auto-grouping above.
            continue;
        }
        match c.review_decision.as_deref() {
            Some("keep_source") => {
                if c.scope == "intra_album" {
                    if let Some(cid) = c.candidate_source_image_id {
                        excluded_image_ids.insert(cid);
                    }
                }
            }
            Some("keep_candidate") => {
                if c.scope == "intra_album" || c.scope == "library" {
                    excluded_image_ids.insert(c.source_image_id);
                }
            }
            Some("keep_all") => {}
            Some("skip_album") => {
                skipped_album_ids.insert(c.source_album_id);
            }
            _ => {}
        }
    }

    let kept_images: Vec<ImportPlanImage> = all_images
        .iter()
        .filter(|img| {
            !excluded_image_ids.contains(&img.id) && !skipped_album_ids.contains(&img.album_id)
        })
        .map(|img: &ImportPlanImageRow| ImportPlanImage {
            image_id: img.id.to_string(),
            source_path: img.source_path.clone(),
            relative_path: img.relative_path.clone(),
            file_size: img.file_size,
            album_name: img.album_name.clone(),
        })
        .collect();

    let total_images = all_images.len() as u32;
    let skipped_album_names: Vec<String> = skipped_album_ids
        .iter()
        .filter_map(|id| album_name_map.get(id).cloned())
        .collect();

    ImportPlan {
        import_run_id,
        total_albums: albums.len() as u32,
        total_images,
        excluded_count: total_images.saturating_sub(kept_images.len() as u32),
        kept_images,
        skipped_albums: skipped_album_names,
    }
}

/// Decode an image from disk, cap its decoded pixel count, downscale to a
/// size-limited thumbnail, and re-encode as JPEG. Returns a data URL. Never
/// returns the full-resolution original.
fn render_thumbnail(
    path: &Path,
    max_dim: u32,
    max_pixels: u64,
    max_source_bytes: u64,
) -> Result<String, AppError> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > max_source_bytes {
        return Err(AppError::IoError(format!(
            "image too large to preview: {} ({} bytes > {})",
            path.display(),
            metadata.len(),
            max_source_bytes
        )));
    }

    // Validate the format by extension before reading, so a non-image file
    // (e.g. a renamed executable) is rejected cheaply.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let _mime = match ext.as_str() {
        "jpg" | "jpeg" | "png" | "webp" => ext.clone(),
        _ => {
            return Err(AppError::Internal(format!(
                "unsupported image format for preview: {}",
                path.display()
            )));
        }
    };

    let bytes = std::fs::read(path)
        .map_err(|e| AppError::IoError(format!("failed to read image {}: {e}", path.display())))?;
    let reader = std::io::Cursor::new(bytes);
    let img = image::ImageReader::new(reader)
        .with_guessed_format()
        .map_err(|e| AppError::ImageError(format!("cannot inspect image: {e}")))?
        .decode()
        .map_err(|e| AppError::ImageError(format!("corrupt or undecodable image: {e}")))?;

    // Cap decoded pixels so a maliciously huge-but-valid image cannot exhaust
    // memory during the resize.
    let (w, h) = (img.width() as u64, img.height() as u64);
    if w.saturating_mul(h) > max_pixels {
        return Err(AppError::ImageError(format!(
            "decoded image too large for preview: {w}x{h} (>{max_pixels} pixels)"
        )));
    }

    // Downscale so neither dimension exceeds max_dim.
    let thumb = if img.width() > max_dim || img.height() > max_dim {
        img.resize(max_dim, max_dim, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .map_err(|e| AppError::ImageError(format!("thumbnail encode failed: {e}")))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());
    Ok(format!("data:image/jpeg;base64,{b64}"))
}

/// Allowed-roots check: a preview path must canonicalize to a location inside
/// the candidate's source root or library root. This blocks path-escape
/// attacks (e.g. a DB row pointing at /etc/passwd or ..\\..\\secrets).
fn path_within_allowed_roots(resolved: &Path, allowed: &[PathBuf]) -> Result<(), AppError> {
    let canon = resolved.canonicalize().map_err(|e| {
        AppError::IoError(format!("cannot canonicalize {}: {e}", resolved.display()))
    })?;
    for root in allowed {
        let root_canon = match root.canonicalize() {
            Ok(c) => c,
            Err(_) => continue, // a root that doesn't exist can't be matched
        };
        if canon.starts_with(&root_canon) {
            return Ok(());
        }
    }
    Err(AppError::Internal(format!(
        "preview path {} is outside the candidate's allowed source/library roots",
        resolved.display()
    )))
}

/// Load an image preview for a review candidate, restricted to persisted records.
///
/// The `image_side` parameter determines which image to preview:
/// - "source": the source image (import_image referenced by candidate)
/// - "candidate": the candidate image (import_image or library_image)
///
/// This function validates that:
/// 1. The candidate exists in the database.
/// 2. The image_side is valid.
/// 3. The resolved path canonicalizes inside the candidate's source root or
///    library root (no path escape).
/// 4. The file is a supported image format.
/// 5. The source file size is within limits.
/// 6. The decoded pixel count is within limits.
/// 7. A size-limited JPEG thumbnail is returned, never the full-resolution
///    original.
pub async fn load_image_preview_by_candidate(
    client: &Client,
    candidate_id: Uuid,
    image_side: &str,
) -> Result<String, AppError> {
    use crate::repositories::import_repository::ImportRepository;
    let detail = ImportRepository::get_review_candidate_detail(client, candidate_id)
        .await?
        .ok_or_else(|| AppError::Internal(format!("candidate {candidate_id} not found")))?;

    let (path, is_library_candidate) = match image_side {
        "source" => (PathBuf::from(&detail.source_image_path), false),
        "candidate" => {
            if let Some(ref p) = detail.candidate_source_image_path {
                (PathBuf::from(p), false)
            } else if let Some(ref p) = detail.candidate_library_image_path {
                (PathBuf::from(p), true)
            } else {
                return Err(AppError::Internal(format!(
                    "candidate {candidate_id} has no candidate image path"
                )));
            }
        }
        _ => {
            return Err(AppError::Internal(format!(
                "invalid image_side: {image_side}; expected 'source' or 'candidate'"
            )));
        }
    };

    // Build the allowed roots: the import run's source_root (for import images)
    // and the library root of the candidate library image (for library images).
    let mut allowed: Vec<PathBuf> = Vec::new();
    // Import run source root.
    let run_row = client
        .query_opt(
            "SELECT ir.source_root FROM import_runs ir
             JOIN import_albums ia ON ia.import_run_id = ir.id
             JOIN import_images ii ON ii.import_album_id = ia.id
             WHERE ii.id = $1",
            &[&detail.source_image_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to query import run root: {e}")))?;
    if let Some(row) = run_row {
        let source_root: String = row.get("source_root");
        allowed.push(PathBuf::from(source_root));
    }
    if is_library_candidate {
        // Library image path is already resolved as root/album_rel/img_rel in
        // get_review_candidate_detail; its allowed root is the library root,
        // which is the parent of the album-relative path. We add the path's
        // own album directory and the broader library root by querying it.
        if let Some(lib_img_id) = detail.candidate_library_image_id {
            let lib_row = client
                .query_opt(
                    "SELECT lr.path AS root_path
                     FROM library_images li
                     JOIN library_albums la ON la.id = li.album_id
                     JOIN library_roots lr ON lr.id = la.library_root_id
                     WHERE li.id = $1",
                    &[&lib_img_id],
                )
                .await
                .map_err(|e| AppError::Internal(format!("failed to query library root: {e}")))?;
            if let Some(row) = lib_row {
                let root_path: String = row.get("root_path");
                allowed.push(PathBuf::from(root_path));
            }
        }
    }

    // Path escape check.
    path_within_allowed_roots(&path, &allowed)?;

    // Validate the path exists.
    if !path.exists() {
        return Err(AppError::IoError(format!(
            "image file not found: {}",
            path.display()
        )));
    }

    render_thumbnail(
        &path,
        PREVIEW_MAX_DIMENSION,
        PREVIEW_MAX_PIXELS,
        PREVIEW_MAX_SOURCE_BYTES,
    )
}

/// Maximum dimension (px) of a generated preview thumbnail.
const PREVIEW_MAX_DIMENSION: u32 = 800;
/// Maximum decoded pixel count for a preview source.
const PREVIEW_MAX_PIXELS: u64 = 50_000_000;
/// Maximum source file size (bytes) for a preview.
const PREVIEW_MAX_SOURCE_BYTES: u64 = 100 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn path_within_allowed_roots_rejects_escape() {
        let tmp = TempDir::new().unwrap();
        let allowed_root = tmp.path().join("src");
        std::fs::create_dir_all(&allowed_root).unwrap();
        let inside = allowed_root.join("a.jpg");
        std::fs::write(&inside, b"x").unwrap();
        let outside = tmp.path().join("secret.txt");
        std::fs::write(&outside, b"x").unwrap();

        assert!(path_within_allowed_roots(&inside, std::slice::from_ref(&allowed_root)).is_ok());
        assert!(
            path_within_allowed_roots(&outside, &[allowed_root]).is_err(),
            "path outside allowed root must be rejected"
        );
    }

    #[test]
    fn path_within_allowed_roots_rejects_traversal() {
        let tmp = TempDir::new().unwrap();
        let allowed_root = tmp.path().join("src");
        std::fs::create_dir_all(&allowed_root).unwrap();
        // A symlink-free traversal: ../secret relative to src.
        let secret = tmp.path().join("secret.txt");
        std::fs::write(&secret, b"x").unwrap();
        let escaped = allowed_root.join("..").join("secret.txt");
        assert!(
            path_within_allowed_roots(&escaped, &[allowed_root]).is_err(),
            "traversal escape must be rejected"
        );
    }

    #[test]
    fn render_thumbnail_rejects_non_image() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("not_image.jpg");
        std::fs::write(&p, b"this is not a real jpeg").unwrap();
        let result = render_thumbnail(&p, 800, 50_000_000, 100 * 1024 * 1024);
        assert!(result.is_err(), "non-image file must be rejected");
    }

    #[test]
    fn render_thumbnail_rejects_corrupt_image() {
        let tmp = TempDir::new().unwrap();
        // Valid extension, garbage content.
        let p = tmp.path().join("corrupt.png");
        std::fs::write(&p, b"\x89PNG\r\n\x1a\nGARBAGE").unwrap();
        let result = render_thumbnail(&p, 800, 50_000_000, 100 * 1024 * 1024);
        assert!(result.is_err(), "corrupt image must be rejected");
    }

    #[test]
    fn render_thumbnail_returns_small_jpeg_data_url() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("ok.png");
        image::RgbImage::new(2000, 2000).save(&p).unwrap();
        let url = render_thumbnail(&p, 800, 50_000_000, 100 * 1024 * 1024).unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));
        // Decoded thumbnail bytes must be much smaller than the original.
        let b64 = &url["data:image/jpeg;base64,".len()..];
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert!(
            bytes.len() < 2000 * 2000 * 3,
            "thumbnail must be downscaled"
        );
    }

    #[test]
    fn render_thumbnail_rejects_unsupported_format() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("doc.txt");
        std::fs::write(&p, b"hello").unwrap();
        let result = render_thumbnail(&p, 800, 50_000_000, 100 * 1024 * 1024);
        assert!(result.is_err());
    }

    #[test]
    fn render_thumbnail_rejects_oversized_source() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("big.png");
        std::fs::write(&p, b"x").unwrap();
        // max_source_bytes = 0 → any non-empty file is too large.
        let result = render_thumbnail(&p, 800, 50_000_000, 0);
        assert!(result.is_err(), "oversized source must be rejected");
    }

    #[test]
    fn review_decision_action_display_parse() {
        let actions = [
            ReviewDecisionAction::KeepSource,
            ReviewDecisionAction::KeepCandidate,
            ReviewDecisionAction::KeepAll,
            ReviewDecisionAction::SkipAlbum,
        ];
        for a in actions {
            assert_eq!(ReviewDecisionAction::from_str_opt(&a.to_string()), Some(a));
        }
    }

    #[test]
    fn review_decision_rejects_unknown() {
        assert_eq!(ReviewDecisionAction::from_str_opt("unknown"), None);
        assert_eq!(ReviewDecisionAction::from_str_opt(""), None);
    }

    fn make_image(id: Uuid, album_id: Uuid, name: &str) -> ImportPlanImageRow {
        ImportPlanImageRow {
            id,
            source_path: format!("/src/{name}"),
            relative_path: name.to_string(),
            file_size: 1000,
            album_id,
            album_name: "album_a".to_string(),
        }
    }

    fn make_album(id: Uuid, name: &str) -> AlbumRow {
        AlbumRow {
            id,
            source_name: name.to_string(),
        }
    }

    #[test]
    fn plan_excludes_auto_duplicates() {
        let album_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let img_a = Uuid::parse_str("00000000-0000-0000-0000-000000000010").unwrap();
        let img_b = Uuid::parse_str("00000000-0000-0000-0000-000000000020").unwrap();
        let cand_id = Uuid::parse_str("00000000-0000-0000-0000-000000000100").unwrap();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_b,
            candidate_source_image_id: Some(img_a),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: Some("auto_duplicate".to_string()),
            review_decision: None,
            source_album_id: album_id,
            blake3_equal: true,
            pixel_hash_equal: true,
            confidence: Some(1.0),
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, img_a.to_string());
        assert_eq!(plan.excluded_count, 1);
    }

    #[test]
    fn plan_keep_source_excludes_candidate_intra_album() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_source".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, img_a.to_string());
    }

    #[test]
    fn plan_keep_candidate_excludes_source_intra_album() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_candidate".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, img_b.to_string());
    }

    #[test]
    fn plan_keep_all_keeps_both() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_all".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 2);
        assert_eq!(plan.excluded_count, 0);
    }

    #[test]
    fn plan_skip_album_excludes_all_images_in_album() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let img_c = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
            make_image(img_c, album_id, "c.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: Some("skip_album".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 0);
        assert_eq!(plan.excluded_count, 3);
        assert_eq!(plan.skipped_albums, vec!["album_a".to_string()]);
    }

    #[test]
    fn plan_library_scope_keep_source_does_not_exclude_library() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![make_image(img_a, album_id, "a.jpg")];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: None,
            candidate_library_image_id: None,
            scope: "library".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_source".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.excluded_count, 0);
    }

    #[test]
    fn plan_library_scope_keep_candidate_excludes_source() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![make_image(img_a, album_id, "a.jpg")];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: None,
            candidate_library_image_id: None,
            scope: "library".to_string(),
            candidate_decision: None,
            review_decision: Some("keep_candidate".to_string()),
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 0);
        assert_eq!(plan.excluded_count, 1);
    }

    #[test]
    fn plan_undecided_review_candidate_not_excluded() {
        let album_id = Uuid::new_v4();
        let img_a = Uuid::new_v4();
        let img_b = Uuid::new_v4();
        let cand_id = Uuid::new_v4();

        let images = vec![
            make_image(img_a, album_id, "a.jpg"),
            make_image(img_b, album_id, "b.jpg"),
        ];
        let candidates = vec![ImportPlanCandidateRow {
            candidate_id: cand_id,
            source_image_id: img_a,
            candidate_source_image_id: Some(img_b),
            candidate_library_image_id: None,
            scope: "intra_album".to_string(),
            candidate_decision: None,
            review_decision: None,
            source_album_id: album_id,
            blake3_equal: false,
            pixel_hash_equal: false,
            confidence: None,
        }];
        let albums = vec![make_album(album_id, "album_a")];

        let plan = build_import_plan("run-1".to_string(), &images, &candidates, &albums);

        assert_eq!(plan.kept_images.len(), 2);
    }

    #[test]
    fn plan_empty_run() {
        let plan = build_import_plan("run-1".to_string(), &[], &[], &[]);
        assert_eq!(plan.kept_images.len(), 0);
        assert_eq!(plan.total_albums, 0);
        assert_eq!(plan.total_images, 0);
        assert_eq!(plan.excluded_count, 0);
    }

    /// Real PostgreSQL review integration test.
    ///
    /// Invocation:
    ///   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test \
    ///       --manifest-path apps/desktop/src-tauri/Cargo.toml \
    ///       --features real-db-tests real_review_decision_persists_and_filters_plan \
    ///       -- --ignored --test-threads=1
    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_review_decision_persists_and_filters_plan() {
        use crate::domain::import_state::{
            DecodeState, DuplicateScope, ImportImageState, ImportRunState, MatchType,
        };
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};
        use crate::repositories::import_repository::{NewDuplicateCandidate, NewImportImage};
        use tempfile::TempDir;

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            eprintln!("IMAGEDB_POSTGRES_BIN not set; skipping real review integration test");
            return;
        }

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let album_path = source_root.join("album_a");
        std::fs::create_dir_all(&album_path).unwrap();
        std::fs::write(album_path.join("source.png"), b"source").unwrap();
        std::fs::write(album_path.join("candidate.png"), b"candidate").unwrap();

        let mut manager = PostgresManager::new(&app_data);
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);
        assert!(
            probe.pgvector_available,
            "diagnostics: {:?}",
            probe.diagnostics
        );

        let (mut client, db_handle) = manager.connect().await.unwrap();
        MigrationRunner::run_pending(&mut client).await.unwrap();

        let library_root_id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        let import_run_id = ImportRepository::create_import_run(
            &client,
            &source_root.display().to_string(),
            library_root_id,
        )
        .await
        .unwrap();
        let album_id = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            &album_path.display().to_string(),
            "album_a",
        )
        .await
        .unwrap();

        let source_id = ImportRepository::insert_import_image(
            &client,
            NewImportImage {
                album_id,
                source_path: album_path.join("source.png").display().to_string(),
                relative_path: "album_a/source.png".to_string(),
                file_size: 6,
                modified_at: None,
                width: Some(10),
                height: Some(10),
                format: Some("png".to_string()),
                decode_state: DecodeState::Decoded,
                blake3: Some(vec![1; 32]),
                pixel_hash: Some(vec![1; 8]),
                gradient_hash: Some(vec![1; 8]),
                block_hash: Some(vec![1; 8]),
                median_hash: Some(vec![1; 8]),
                fingerprint_version: Some("test".to_string()),
                state: ImportImageState::Fingerprinted,
            },
        )
        .await
        .unwrap();
        let candidate_id = ImportRepository::insert_import_image(
            &client,
            NewImportImage {
                album_id,
                source_path: album_path.join("candidate.png").display().to_string(),
                relative_path: "album_a/candidate.png".to_string(),
                file_size: 9,
                modified_at: None,
                width: Some(10),
                height: Some(10),
                format: Some("png".to_string()),
                decode_state: DecodeState::Decoded,
                blake3: Some(vec![2; 32]),
                pixel_hash: Some(vec![2; 8]),
                gradient_hash: Some(vec![2; 8]),
                block_hash: Some(vec![2; 8]),
                median_hash: Some(vec![2; 8]),
                fingerprint_version: Some("test".to_string()),
                state: ImportImageState::Fingerprinted,
            },
        )
        .await
        .unwrap();

        let review_candidate_id = ImportRepository::insert_duplicate_candidate(
            &client,
            NewDuplicateCandidate {
                import_run_id,
                source_image_id: source_id,
                candidate_source_image_id: Some(candidate_id),
                candidate_library_image_id: None,
                scope: DuplicateScope::IntraAlbum,
                match_type: MatchType::PerceptualSimilar,
                blake3_equal: false,
                pixel_hash_equal: false,
                gradient_distance: Some(10),
                block_distance: Some(11),
                median_distance: Some(12),
                transform_type: Some("identity".to_string()),
                confidence: Some(0.75),
                decision: None,
                decision_source: None,
            },
        )
        .await
        .unwrap();

        ImportRepository::update_import_run_state(
            &client,
            import_run_id,
            &ImportRunState::Completed,
        )
        .await
        .unwrap();

        let queue = get_review_queue(&client, import_run_id).await.unwrap();
        assert_eq!(queue.len(), 1);
        assert!(!queue[0].has_decision);

        let blocked_plan = generate_import_plan(&client, import_run_id).await;
        assert!(blocked_plan.is_err());

        submit_decision(
            &client,
            review_candidate_id,
            ReviewDecisionAction::KeepSource,
        )
        .await
        .unwrap();
        submit_decision(
            &client,
            review_candidate_id,
            ReviewDecisionAction::KeepSource,
        )
        .await
        .unwrap();
        let conflicting = submit_decision(
            &client,
            review_candidate_id,
            ReviewDecisionAction::KeepCandidate,
        )
        .await;
        assert!(conflicting.is_err());

        let progress = get_review_progress(&client, import_run_id).await.unwrap();
        assert_eq!(progress.total_review_candidates, 1);
        assert_eq!(progress.decided_count, 1);
        assert!(progress.all_decided);

        let queue = get_review_queue(&client, import_run_id).await.unwrap();
        assert_eq!(queue.len(), 1);
        assert!(queue[0].has_decision);

        let plan = generate_import_plan(&client, import_run_id).await.unwrap();
        assert_eq!(plan.kept_images.len(), 1);
        assert_eq!(plan.kept_images[0].image_id, source_id.to_string());
        assert_eq!(plan.excluded_count, 1);

        drop(client);
        db_handle.abort();
        manager.shutdown().await.unwrap();
    }
}

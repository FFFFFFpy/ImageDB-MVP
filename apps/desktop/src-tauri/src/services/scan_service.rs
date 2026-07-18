use crate::domain::import_state::{
    Decision, DecisionSource, DecodeState, DuplicateScope, ImportImageState, ImportRunState,
    MatchType, ScanProgress, TransformType, SUPPORTED_IMAGE_EXTENSIONS,
};
use crate::error::AppError;
use crate::infrastructure::image_fingerprint_v2::{
    compute_double_gradient_for_transform, fingerprint_image, fingerprint_worker_count,
    hamming_distance, inspect_image_dimensions, recall_radius, weighted_similarity,
    BlockHashVariant, BLOCK_AUTO_DISTANCE_RATIO, BLOCK_REVIEW_DISTANCE_RATIO,
    DOUBLE_GRADIENT_AUTO_DISTANCE_RATIO, DOUBLE_GRADIENT_REVIEW_DISTANCE_RATIO,
    FINGERPRINT_VERSION, LARGE_IMAGE_PIXEL_THRESHOLD,
};
use crate::infrastructure::library_fingerprint_index::{
    BlockTransformMatches, HammingBkTree, LibraryFingerprintIndex, LibraryRecallMatch,
    LibraryRecallResult,
};
use crate::infrastructure::postgres::{DatabaseOperationLock, PostgresManager};
use crate::infrastructure::settings::SettingsStore;
use crate::repositories::import_repository::{
    ImportRepository, NewDuplicateCandidate, NewImportImage, RunExactFingerprintRow,
};
use crate::services::source_snapshot_service::{
    capture_source_album_snapshot_with_cancel, verify_source_album_snapshot_with_cancel,
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio_postgres::Client;
use uuid::Uuid;

pub const SCAN_PROGRESS_EVENT: &str = "scan-progress";

#[derive(Debug, Clone, Serialize)]
pub struct ScanProgressEvent {
    pub state: String,
    pub import_run_id: Option<String>,
    pub current_stage: String,
    pub current_album: Option<String>,
    pub processed_images: u32,
    pub total_albums: u32,
    pub total_images: u32,
    pub duplicate_count: u32,
    pub error_count: u32,
    pub errors: Vec<String>,
}

struct AlbumEntry {
    name: String,
    path: PathBuf,
}

struct ScannedImage {
    source_path: PathBuf,
    relative_path: String,
    file_size: i64,
    modified_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn fingerprint_worker_limit() -> Arc<Semaphore> {
    static LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    LIMIT
        .get_or_init(|| Arc::new(Semaphore::new(fingerprint_worker_count())))
        .clone()
}

fn large_image_decode_limit() -> Arc<Semaphore> {
    static LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    LIMIT.get_or_init(|| Arc::new(Semaphore::new(1))).clone()
}

fn requires_serial_large_image_decode(pixel_count: u64) -> bool {
    pixel_count >= LARGE_IMAGE_PIXEL_THRESHOLD
}

async fn fingerprint_scanned_image(
    img: ScannedImage,
) -> Result<(ScannedImage, Result<FingerprintedData, AppError>), AppError> {
    let inspect_path = img.source_path.clone();
    let inspected = tokio::task::spawn_blocking(move || inspect_image_dimensions(&inspect_path))
        .await
        .map_err(|e| AppError::Internal(format!("image dimension worker failed: {e}")))?;
    let pixel_count = match inspected {
        Ok((_, _, pixels)) => pixels,
        Err(error) => return Ok((img, Err(error))),
    };
    let large_permit = if requires_serial_large_image_decode(pixel_count) {
        Some(
            large_image_decode_limit()
                .acquire_owned()
                .await
                .map_err(|e| AppError::Internal(format!("large image decode limit closed: {e}")))?,
        )
    } else {
        None
    };
    let permit = fingerprint_worker_limit()
        .acquire_owned()
        .await
        .map_err(|e| AppError::Internal(format!("fingerprint worker pool closed: {e}")))?;
    let path = img.source_path.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _large_permit = large_permit;
        fingerprint_image_sync(&path)
    })
    .await
    .map_err(|e| AppError::Internal(format!("fingerprint worker failed: {e}")))?;
    Ok((img, result))
}

struct FingerprintedData {
    file_size: u64,
    width: u32,
    height: u32,
    format: String,
    blake3_bytes: Vec<u8>,
    pixel_hash_bytes: Vec<u8>,
    block_hash_16: Vec<u8>,
    double_gradient_hash_32: Vec<u8>,
    perceptual_eligible: bool,
    block_variants: Vec<BlockHashVariant>,
    fine_thumbnail_32: image::GrayImage,
}

struct AlbumImageEntry {
    image_db_id: Uuid,
    fp: FingerprintedData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RunExactFingerprint {
    image_id: Uuid,
    album_id: Uuid,
    file_size: i64,
    blake3: Vec<u8>,
    pixel_hash: Vec<u8>,
}

impl RunExactFingerprint {
    fn from_album_image(album_id: Uuid, image: &AlbumImageEntry) -> Self {
        Self {
            image_id: image.image_db_id,
            album_id,
            file_size: image.fp.file_size as i64,
            blake3: image.fp.blake3_bytes.clone(),
            pixel_hash: image.fp.pixel_hash_bytes.clone(),
        }
    }
}

#[derive(Debug, Default)]
struct RunExactRepresentativeIndex {
    file_representatives: HashMap<(i64, Vec<u8>), RunExactFingerprint>,
    pixel_representatives: HashMap<Vec<u8>, RunExactFingerprint>,
}

impl RunExactRepresentativeIndex {
    fn from_rows(rows: Vec<RunExactFingerprintRow>) -> Self {
        let mut index = Self::default();
        for row in rows {
            index.add(RunExactFingerprint {
                image_id: row.id,
                album_id: row.album_id,
                file_size: row.file_size,
                blake3: row.blake3,
                pixel_hash: row.pixel_hash,
            });
        }
        index
    }

    fn add_album(&mut self, album_id: Uuid, images: &[AlbumImageEntry]) {
        for image in images {
            self.add(RunExactFingerprint::from_album_image(album_id, image));
        }
    }

    fn add(&mut self, fingerprint: RunExactFingerprint) {
        let file_key = (fingerprint.file_size, fingerprint.blake3.clone());
        match self.file_representatives.get_mut(&file_key) {
            Some(current) if fingerprint.image_id < current.image_id => {
                *current = fingerprint.clone();
            }
            None => {
                self.file_representatives
                    .insert(file_key, fingerprint.clone());
            }
            _ => {}
        }
        match self.pixel_representatives.get_mut(&fingerprint.pixel_hash) {
            Some(current) if fingerprint.image_id < current.image_id => {
                *current = fingerprint.clone();
            }
            None => {
                self.pixel_representatives
                    .insert(fingerprint.pixel_hash.clone(), fingerprint);
            }
            _ => {}
        }
    }
}

struct AlbumDetectionContext<'a> {
    client: &'a Client,
    import_run_id: Uuid,
    album_id: Uuid,
    cancelled: &'a AtomicBool,
    library_index: &'a Arc<RwLock<Option<LibraryFingerprintIndex>>>,
    run_exact_index: &'a RunExactRepresentativeIndex,
}

async fn emit_progress(progress: &ScanProgressEvent, tracker: &Mutex<ScanProgress>) {
    let mut guard = tracker.lock().await;
    *guard = ScanProgress {
        state: progress.state.clone(),
        import_run_id: progress.import_run_id.clone(),
        current_stage: progress.current_stage.clone(),
        current_album: progress.current_album.clone(),
        processed_images: progress.processed_images,
        total_albums: progress.total_albums,
        total_images: progress.total_images,
        duplicate_count: progress.duplicate_count,
        error_count: progress.error_count,
        errors: progress.errors.clone(),
    };
}

fn scan_directory_for_albums(source_root: &Path) -> Result<Vec<AlbumEntry>, AppError> {
    scan_directory_for_albums_with_cancel(source_root, None)
}

fn scan_directory_for_albums_with_cancel(
    source_root: &Path,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<AlbumEntry>, AppError> {
    let entries = std::fs::read_dir(source_root)?;
    let mut albums = Vec::new();
    for entry in entries {
        if cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(AppError::Internal("scan cancelled".to_string()));
        }
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            albums.push(AlbumEntry {
                name,
                path: entry.path(),
            });
        }
    }
    albums.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(albums)
}

#[cfg(test)]
fn scan_album_for_images(
    album_path: &Path,
    album_name: &str,
) -> Result<Vec<ScannedImage>, AppError> {
    scan_album_for_images_with_cancel(album_path, album_name, None)
}

fn scan_album_for_images_with_cancel(
    album_path: &Path,
    album_name: &str,
    cancelled: Option<&AtomicBool>,
) -> Result<Vec<ScannedImage>, AppError> {
    let mut images = Vec::new();
    walk_album_images(album_path, album_path, album_name, cancelled, &mut images)?;
    images.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(images)
}

fn walk_album_images(
    dir: &Path,
    album_path: &Path,
    album_name: &str,
    cancelled: Option<&AtomicBool>,
    images: &mut Vec<ScannedImage>,
) -> Result<(), AppError> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        if cancelled.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(AppError::Internal("scan cancelled".to_string()));
        }
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            walk_album_images(&path, album_path, album_name, cancelled, images)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !SUPPORTED_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let metadata = std::fs::metadata(&path)?;
        let file_size = metadata.len() as i64;
        let modified_at = metadata
            .modified()
            .ok()
            .map(chrono::DateTime::<chrono::Utc>::from);
        let image_relative_path = path.strip_prefix(album_path).map_err(|e| {
            AppError::Internal(format!(
                "failed to derive relative image path for {}: {e}",
                path.display()
            ))
        })?;
        let relative_path = format!(
            "{}/{}",
            album_name,
            normalize_relative_path(image_relative_path)
        );
        images.push(ScannedImage {
            source_path: path,
            relative_path,
            file_size,
            modified_at,
        });
    }
    Ok(())
}

fn normalize_relative_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Debug)]
struct PerceptualEvidence {
    perceptual_eligible: bool,
    block_distance: i32,
    double_gradient_distance: i32,
    block_distance_ratio: f64,
    double_gradient_distance_ratio: f64,
    transform_type: TransformType,
    confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CandidateKey {
    ImportPair(Uuid, Uuid),
    LibraryPair(Uuid, Uuid),
}

#[derive(Debug, Default)]
struct AlbumAnalysisMetrics {
    image_count: usize,
    fingerprint_success_count: usize,
    fingerprint_failure_count: usize,
    perceptual_ineligible_count: usize,
    file_exact_candidate_count: usize,
    pixel_exact_candidate_count: usize,
    perceptual_recall_candidate_count: usize,
    perceptual_auto_duplicate_count: usize,
    review_candidate_count: usize,
    truncated_image_count: usize,
    fingerprint_ms: u128,
    intra_detection_ms: u128,
    library_recall_ms: u128,
    fine_verification_ms: u128,
    database_write_ms: u128,
}

fn candidate_key(candidate: &mut NewDuplicateCandidate) -> Result<CandidateKey, AppError> {
    if let Some(candidate_source_id) = candidate.candidate_source_image_id {
        let (left, right) = if candidate.source_image_id <= candidate_source_id {
            (candidate.source_image_id, candidate_source_id)
        } else {
            (candidate_source_id, candidate.source_image_id)
        };
        candidate.source_image_id = left;
        candidate.candidate_source_image_id = Some(right);
        return Ok(CandidateKey::ImportPair(left, right));
    }
    if let Some(library_id) = candidate.candidate_library_image_id {
        return Ok(CandidateKey::LibraryPair(
            candidate.source_image_id,
            library_id,
        ));
    }
    Err(AppError::Internal(
        "duplicate candidate has no candidate image".to_string(),
    ))
}

fn candidate_priority(match_type: &MatchType) -> u8 {
    match match_type {
        MatchType::FileExact => 0,
        MatchType::PixelExact => 1,
        MatchType::PerceptualNear => 2,
        MatchType::PerceptualSimilar => 3,
    }
}

fn insert_best_candidate(
    candidates: &mut HashMap<CandidateKey, NewDuplicateCandidate>,
    mut candidate: NewDuplicateCandidate,
) -> Result<(), AppError> {
    let key = candidate_key(&mut candidate)?;
    let replace = candidates
        .get(&key)
        .map(|current| {
            candidate_priority(&candidate.match_type) < candidate_priority(&current.match_type)
        })
        .unwrap_or(true);
    if replace {
        candidates.insert(key, candidate);
    }
    Ok(())
}

fn insert_run_exact_representative_candidates(
    import_run_id: Uuid,
    album_id: Uuid,
    images: &[&AlbumImageEntry],
    run_index: &RunExactRepresentativeIndex,
    candidates: &mut HashMap<CandidateKey, NewDuplicateCandidate>,
) -> Result<(), AppError> {
    let mut file_groups: HashMap<(i64, Vec<u8>), Vec<RunExactFingerprint>> = HashMap::new();
    let mut pixel_groups: HashMap<Vec<u8>, Vec<RunExactFingerprint>> = HashMap::new();
    for &image in images {
        let fingerprint = RunExactFingerprint::from_album_image(album_id, image);
        file_groups
            .entry((fingerprint.file_size, fingerprint.blake3.clone()))
            .or_default()
            .push(fingerprint.clone());
        pixel_groups
            .entry(fingerprint.pixel_hash.clone())
            .or_default()
            .push(fingerprint);
    }

    for (key, group) in file_groups {
        insert_exact_star(
            import_run_id,
            group,
            run_index.file_representatives.get(&key),
            MatchType::FileExact,
            candidates,
        )?;
    }
    for (key, group) in pixel_groups {
        insert_exact_star(
            import_run_id,
            group,
            run_index.pixel_representatives.get(&key),
            MatchType::PixelExact,
            candidates,
        )?;
    }
    Ok(())
}

fn insert_exact_star(
    import_run_id: Uuid,
    mut current_members: Vec<RunExactFingerprint>,
    prior_representative: Option<&RunExactFingerprint>,
    match_type: MatchType,
    candidates: &mut HashMap<CandidateKey, NewDuplicateCandidate>,
) -> Result<(), AppError> {
    if let Some(prior) = prior_representative {
        if !current_members
            .iter()
            .any(|member| member.image_id == prior.image_id)
        {
            current_members.push(prior.clone());
        }
    }
    let Some(representative) = current_members
        .iter()
        .min_by_key(|member| member.image_id)
        .cloned()
    else {
        return Ok(());
    };

    for member in current_members {
        if member.image_id == representative.image_id {
            continue;
        }
        let is_file_exact =
            member.file_size == representative.file_size && member.blake3 == representative.blake3;
        let is_pixel_exact = member.pixel_hash == representative.pixel_hash;
        insert_best_candidate(
            candidates,
            NewDuplicateCandidate {
                import_run_id,
                source_image_id: member.image_id,
                candidate_source_image_id: Some(representative.image_id),
                candidate_library_image_id: None,
                scope: if member.album_id == representative.album_id {
                    DuplicateScope::IntraAlbum
                } else {
                    DuplicateScope::CrossAlbum
                },
                match_type: match_type.clone(),
                blake3_equal: is_file_exact,
                pixel_hash_equal: is_pixel_exact,
                block_distance: None,
                double_gradient_distance: None,
                block_distance_ratio: None,
                double_gradient_distance_ratio: None,
                transform_type: None,
                confidence: Some(1.0),
                decision: Some(Decision::AutoDuplicate),
                decision_source: Some(DecisionSource::ExactRule),
            },
        )?;
    }
    Ok(())
}

fn classify_perceptual(
    evidence: &PerceptualEvidence,
) -> Option<(MatchType, Option<Decision>, Option<DecisionSource>)> {
    if !evidence.perceptual_eligible {
        return None;
    }
    if evidence.block_distance_ratio <= BLOCK_AUTO_DISTANCE_RATIO
        && evidence.double_gradient_distance_ratio <= DOUBLE_GRADIENT_AUTO_DISTANCE_RATIO
    {
        return Some((
            MatchType::PerceptualNear,
            Some(Decision::AutoDuplicate),
            Some(DecisionSource::PerceptualRule),
        ));
    }
    if evidence.block_distance_ratio <= BLOCK_REVIEW_DISTANCE_RATIO
        && evidence.double_gradient_distance_ratio <= DOUBLE_GRADIENT_REVIEW_DISTANCE_RATIO
    {
        return Some((MatchType::PerceptualSimilar, None, None));
    }
    None
}

fn select_best_fine_transform(
    source: &FingerprintedData,
    candidate_perceptual_eligible: bool,
    candidate_block_hash: &[u8],
    candidate_fine_hash: &[u8],
    transforms: &[TransformType],
    fine_hash_cache: &mut HashMap<String, Vec<u8>>,
) -> Result<PerceptualEvidence, AppError> {
    let mut best: Option<PerceptualEvidence> = None;
    for &transform in transforms {
        let variant = source
            .block_variants
            .iter()
            .find(|variant| variant.transform == transform)
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "missing BlockHash variant for recalled transform {transform}"
                ))
            })?;
        let block_distance = hamming_distance(&variant.hash, candidate_block_hash)?;
        let transformed_fine = fine_hash_cache
            .entry(transform.to_string())
            .or_insert_with(|| {
                compute_double_gradient_for_transform(&source.fine_thumbnail_32, transform)
            });
        let fine_distance = hamming_distance(transformed_fine, candidate_fine_hash)?;
        let evidence = PerceptualEvidence {
            perceptual_eligible: source.perceptual_eligible && candidate_perceptual_eligible,
            block_distance: block_distance.raw_distance as i32,
            double_gradient_distance: fine_distance.raw_distance as i32,
            block_distance_ratio: block_distance.normalized_distance,
            double_gradient_distance_ratio: fine_distance.normalized_distance,
            transform_type: transform,
            confidence: weighted_similarity(
                block_distance.normalized_distance,
                fine_distance.normalized_distance,
            ),
        };
        let replace = best
            .as_ref()
            .map(|current| {
                (
                    evidence.double_gradient_distance,
                    evidence.block_distance,
                    evidence.transform_type.to_string(),
                ) < (
                    current.double_gradient_distance,
                    current.block_distance,
                    current.transform_type.to_string(),
                )
            })
            .unwrap_or(true);
        if replace {
            best = Some(evidence);
        }
    }
    best.ok_or_else(|| {
        AppError::Internal("perceptual recall candidate has no tied transforms".to_string())
    })
}

fn fingerprint_image_sync(path: &Path) -> Result<FingerprintedData, AppError> {
    let fingerprint = fingerprint_image(path)?;
    Ok(FingerprintedData {
        file_size: fingerprint.file_size,
        width: fingerprint.width,
        height: fingerprint.height,
        format: fingerprint.format,
        blake3_bytes: fingerprint.blake3,
        pixel_hash_bytes: fingerprint.pixel_hash,
        block_hash_16: fingerprint.block_hash_16,
        double_gradient_hash_32: fingerprint.double_gradient_hash_32,
        perceptual_eligible: fingerprint.perceptual_eligible,
        block_variants: fingerprint.block_variants,
        fine_thumbnail_32: fingerprint.fine_thumbnail_32,
    })
}

async fn ensure_library_index(
    client: &Client,
    cache: &Arc<RwLock<Option<LibraryFingerprintIndex>>>,
) -> Result<(), AppError> {
    if cache.read().await.is_some() {
        return Ok(());
    }
    let mut guard = cache.write().await;
    if guard.is_some() {
        return Ok(());
    }
    let rows = ImportRepository::get_library_images_for_comparison(client).await?;
    match LibraryFingerprintIndex::build(&rows) {
        Ok(index) => {
            tracing::info!(
                fingerprint_version = index.fingerprint_version,
                image_count = index.image_count,
                unique_hash_count = index.unique_hash_count(),
                "library fingerprint index built"
            );
            *guard = Some(index);
            Ok(())
        }
        Err(error) => {
            *guard = None;
            tracing::error!(error = %error, "library fingerprint index build failed; cache remains invalid");
            Err(error)
        }
    }
}

pub fn validate_source_directory(path: &str) -> Result<ScanProgress, AppError> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(AppError::Internal(format!(
            "directory does not exist: {path}"
        )));
    }
    if !p.is_dir() {
        return Err(AppError::Internal(format!("not a directory: {path}")));
    }
    Ok(ScanProgress::idle())
}

async fn detect_album_duplicates(
    ctx: &AlbumDetectionContext<'_>,
    images: &[&AlbumImageEntry],
    duplicate_count: &mut u32,
    progress: &mut ScanProgressEvent,
    metrics: &mut AlbumAnalysisMetrics,
) -> Result<(), AppError> {
    let mut candidates: HashMap<CandidateKey, NewDuplicateCandidate> = HashMap::new();
    let mut intra_tree = HammingBkTree::default();
    let mut block_hash_to_images: HashMap<Vec<u8>, Vec<&AlbumImageEntry>> = HashMap::new();
    let intra_started = Instant::now();

    // File and pixel exact groups share run-level stable representatives.
    // Every new member adds one representative edge, so a k-member group has
    // k - 1 stored edges even when it spans many albums or a resumed scan.
    insert_run_exact_representative_candidates(
        ctx.import_run_id,
        ctx.album_id,
        images,
        ctx.run_exact_index,
        &mut candidates,
    )?;

    for current in images {
        if ctx.cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if !current.fp.perceptual_eligible {
            metrics.perceptual_ineligible_count += 1;
            continue;
        }

        let mut recalled: HashMap<Uuid, (&AlbumImageEntry, BlockTransformMatches)> = HashMap::new();
        for variant in &current.fp.block_variants {
            for (base_hash, distance) in intra_tree.search(
                &variant.hash,
                recall_radius((variant.hash.len() * 8) as u32),
            )? {
                if let Some(previous) = block_hash_to_images.get(&base_hash) {
                    for prior in previous {
                        if let Some((_, matches)) = recalled.get_mut(&prior.image_db_id) {
                            matches.consider(distance, variant.transform);
                        } else {
                            recalled.insert(
                                prior.image_db_id,
                                (
                                    *prior,
                                    BlockTransformMatches::new(distance, variant.transform),
                                ),
                            );
                        }
                    }
                }
            }
        }

        metrics.perceptual_recall_candidate_count += recalled.len();
        let mut fine_hash_cache: HashMap<String, Vec<u8>> = HashMap::new();
        for (prior, block_matches) in recalled.into_values() {
            if (current.fp.file_size == prior.fp.file_size
                && current.fp.blake3_bytes == prior.fp.blake3_bytes)
                || current.fp.pixel_hash_bytes == prior.fp.pixel_hash_bytes
            {
                continue;
            }
            let pair = if current.image_db_id <= prior.image_db_id {
                CandidateKey::ImportPair(current.image_db_id, prior.image_db_id)
            } else {
                CandidateKey::ImportPair(prior.image_db_id, current.image_db_id)
            };
            if candidates.contains_key(&pair) {
                continue;
            }
            let fine_started = Instant::now();
            let evidence = select_best_fine_transform(
                &current.fp,
                prior.fp.perceptual_eligible,
                &prior.fp.block_hash_16,
                &prior.fp.double_gradient_hash_32,
                &block_matches.transforms,
                &mut fine_hash_cache,
            )?;
            metrics.fine_verification_ms += fine_started.elapsed().as_millis();
            if let Some((match_type, decision, decision_source)) = classify_perceptual(&evidence) {
                insert_best_candidate(
                    &mut candidates,
                    NewDuplicateCandidate {
                        import_run_id: ctx.import_run_id,
                        source_image_id: current.image_db_id,
                        candidate_source_image_id: Some(prior.image_db_id),
                        candidate_library_image_id: None,
                        scope: DuplicateScope::IntraAlbum,
                        match_type,
                        blake3_equal: false,
                        pixel_hash_equal: false,
                        block_distance: Some(evidence.block_distance),
                        double_gradient_distance: Some(evidence.double_gradient_distance),
                        block_distance_ratio: Some(evidence.block_distance_ratio),
                        double_gradient_distance_ratio: Some(
                            evidence.double_gradient_distance_ratio,
                        ),
                        transform_type: Some(evidence.transform_type.to_string()),
                        confidence: Some(evidence.confidence),
                        decision,
                        decision_source,
                    },
                )?;
            }
        }

        intra_tree.insert(current.fp.block_hash_16.clone())?;
        block_hash_to_images
            .entry(current.fp.block_hash_16.clone())
            .or_default()
            .push(current);
    }
    metrics.intra_detection_ms = intra_started.elapsed().as_millis();

    ensure_library_index(ctx.client, ctx.library_index).await?;
    for entry in images {
        if ctx.cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        let recall_started = Instant::now();
        let (exact_file_ids, exact_pixel_ids, recall_result) = {
            let guard = ctx.library_index.read().await;
            let index = guard.as_ref().ok_or_else(|| {
                AppError::Internal("library fingerprint index unexpectedly invalid".to_string())
            })?;
            let exact_file_ids =
                index.exact_file_matches(entry.fp.file_size as i64, &entry.fp.blake3_bytes);
            let exact_pixel_ids = index.exact_pixel_matches(&entry.fp.pixel_hash_bytes);
            let recall = if entry.fp.perceptual_eligible {
                index.recall(
                    &entry.fp.block_variants,
                    recall_radius((entry.fp.block_hash_16.len() * 8) as u32),
                )?
            } else {
                LibraryRecallResult {
                    matches: Vec::new(),
                    truncated: false,
                }
            };

            for library_id in &exact_file_ids {
                let pixel_equal = index
                    .metadata(*library_id)
                    .is_some_and(|metadata| metadata.pixel_hash == entry.fp.pixel_hash_bytes);
                insert_best_candidate(
                    &mut candidates,
                    NewDuplicateCandidate {
                        import_run_id: ctx.import_run_id,
                        source_image_id: entry.image_db_id,
                        candidate_source_image_id: None,
                        candidate_library_image_id: Some(*library_id),
                        scope: DuplicateScope::Library,
                        match_type: MatchType::FileExact,
                        blake3_equal: true,
                        pixel_hash_equal: pixel_equal,
                        block_distance: None,
                        double_gradient_distance: None,
                        block_distance_ratio: None,
                        double_gradient_distance_ratio: None,
                        transform_type: None,
                        confidence: Some(1.0),
                        decision: Some(Decision::AutoDuplicate),
                        decision_source: Some(DecisionSource::ExactRule),
                    },
                )?;
            }
            for library_id in &exact_pixel_ids {
                insert_best_candidate(
                    &mut candidates,
                    NewDuplicateCandidate {
                        import_run_id: ctx.import_run_id,
                        source_image_id: entry.image_db_id,
                        candidate_source_image_id: None,
                        candidate_library_image_id: Some(*library_id),
                        scope: DuplicateScope::Library,
                        match_type: MatchType::PixelExact,
                        blake3_equal: false,
                        pixel_hash_equal: true,
                        block_distance: None,
                        double_gradient_distance: None,
                        block_distance_ratio: None,
                        double_gradient_distance_ratio: None,
                        transform_type: None,
                        confidence: Some(1.0),
                        decision: Some(Decision::AutoDuplicate),
                        decision_source: Some(DecisionSource::ExactRule),
                    },
                )?;
            }
            (exact_file_ids, exact_pixel_ids, recall)
        };
        if recall_result.truncated {
            metrics.truncated_image_count += 1;
            tracing::warn!(
                %ctx.import_run_id,
                %ctx.album_id,
                image_id = %entry.image_db_id,
                "library fingerprint recall candidate set truncated"
            );
        }
        let exact_ids: HashSet<_> = exact_file_ids.into_iter().chain(exact_pixel_ids).collect();
        let recalled: Vec<LibraryRecallMatch> = recall_result
            .matches
            .into_iter()
            .filter(|candidate| !exact_ids.contains(&candidate.image_id))
            .collect();
        metrics.perceptual_recall_candidate_count += recalled.len();
        let recalled_ids: Vec<Uuid> = recalled
            .iter()
            .map(|candidate| candidate.image_id)
            .collect();
        let library_rows =
            ImportRepository::find_library_images_by_ids(ctx.client, &recalled_ids).await?;
        metrics.library_recall_ms += recall_started.elapsed().as_millis();
        let recall_by_id: HashMap<Uuid, LibraryRecallMatch> = recalled
            .into_iter()
            .map(|candidate| (candidate.image_id, candidate))
            .collect();
        let mut fine_hash_cache: HashMap<String, Vec<u8>> = HashMap::new();
        for library in library_rows {
            let Some(recall) = recall_by_id.get(&library.id) else {
                continue;
            };
            let library_block_hash = library.block_hash_16.as_ref().ok_or_else(|| {
                AppError::Internal(format!(
                    "recalled V2 library image {} is missing BlockHash 16x16",
                    library.id
                ))
            })?;
            let library_fine_hash = library.double_gradient_hash_32.as_ref().ok_or_else(|| {
                AppError::Internal(format!(
                    "recalled V2 library image {} is missing DoubleGradient 32x32",
                    library.id
                ))
            })?;
            let fine_started = Instant::now();
            let evidence = select_best_fine_transform(
                &entry.fp,
                library.perceptual_eligible,
                library_block_hash,
                library_fine_hash,
                &recall.transforms,
                &mut fine_hash_cache,
            )?;
            metrics.fine_verification_ms += fine_started.elapsed().as_millis();
            if let Some((match_type, decision, decision_source)) = classify_perceptual(&evidence) {
                insert_best_candidate(
                    &mut candidates,
                    NewDuplicateCandidate {
                        import_run_id: ctx.import_run_id,
                        source_image_id: entry.image_db_id,
                        candidate_source_image_id: None,
                        candidate_library_image_id: Some(library.id),
                        scope: DuplicateScope::Library,
                        match_type,
                        blake3_equal: false,
                        pixel_hash_equal: false,
                        block_distance: Some(evidence.block_distance),
                        double_gradient_distance: Some(evidence.double_gradient_distance),
                        block_distance_ratio: Some(evidence.block_distance_ratio),
                        double_gradient_distance_ratio: Some(
                            evidence.double_gradient_distance_ratio,
                        ),
                        transform_type: Some(evidence.transform_type.to_string()),
                        confidence: Some(evidence.confidence),
                        decision,
                        decision_source,
                    },
                )?;
            }
        }
    }

    let mut candidate_values: Vec<_> = candidates.into_values().collect();
    candidate_values.sort_by(|left, right| {
        left.source_image_id
            .cmp(&right.source_image_id)
            .then_with(|| {
                left.candidate_source_image_id
                    .cmp(&right.candidate_source_image_id)
            })
            .then_with(|| {
                left.candidate_library_image_id
                    .cmp(&right.candidate_library_image_id)
            })
            .then_with(|| {
                left.match_type
                    .to_string()
                    .cmp(&right.match_type.to_string())
            })
    });
    metrics.file_exact_candidate_count = candidate_values
        .iter()
        .filter(|candidate| candidate.match_type == MatchType::FileExact)
        .count();
    metrics.pixel_exact_candidate_count = candidate_values
        .iter()
        .filter(|candidate| candidate.match_type == MatchType::PixelExact)
        .count();
    metrics.perceptual_auto_duplicate_count = candidate_values
        .iter()
        .filter(|candidate| {
            candidate.match_type == MatchType::PerceptualNear
                && candidate.decision == Some(Decision::AutoDuplicate)
        })
        .count();
    metrics.review_candidate_count = candidate_values
        .iter()
        .filter(|candidate| candidate.decision.is_none())
        .count();

    let database_started = Instant::now();
    let inserted =
        ImportRepository::upsert_duplicate_candidates_batch(ctx.client, &candidate_values).await?;
    metrics.database_write_ms = database_started.elapsed().as_millis();
    *duplicate_count = duplicate_count.saturating_add(inserted as u32);
    progress.duplicate_count = *duplicate_count;
    Ok(())
}

pub async fn run_scan(
    postgres_manager: Arc<Mutex<PostgresManager>>,
    settings: Arc<Mutex<SettingsStore>>,
    library_index: Arc<RwLock<Option<LibraryFingerprintIndex>>>,
    source_root: String,
    cancelled: Arc<AtomicBool>,
    progress_tracker: Arc<Mutex<ScanProgress>>,
) -> Result<ScanProgress, AppError> {
    run_scan_inner(
        postgres_manager,
        settings,
        library_index,
        source_root,
        cancelled,
        progress_tracker,
        None,
    )
    .await
}

pub async fn run_scan_for_import_run(
    postgres_manager: Arc<Mutex<PostgresManager>>,
    settings: Arc<Mutex<SettingsStore>>,
    library_index: Arc<RwLock<Option<LibraryFingerprintIndex>>>,
    source_root: String,
    import_run_id: Uuid,
    cancelled: Arc<AtomicBool>,
    progress_tracker: Arc<Mutex<ScanProgress>>,
) -> Result<ScanProgress, AppError> {
    run_scan_inner(
        postgres_manager,
        settings,
        library_index,
        source_root,
        cancelled,
        progress_tracker,
        Some(import_run_id),
    )
    .await
}

async fn run_scan_inner(
    postgres_manager: Arc<Mutex<PostgresManager>>,
    settings: Arc<Mutex<SettingsStore>>,
    library_index: Arc<RwLock<Option<LibraryFingerprintIndex>>>,
    source_root: String,
    cancelled: Arc<AtomicBool>,
    progress_tracker: Arc<Mutex<ScanProgress>>,
    resume_import_run_id: Option<Uuid>,
) -> Result<ScanProgress, AppError> {
    let started_at = Instant::now();
    tracing::info!(source_root = %source_root, "scan started");

    let mut progress = ScanProgressEvent {
        state: "running".to_string(),
        import_run_id: None,
        current_stage: "scanning".to_string(),
        current_album: None,
        processed_images: 0,
        total_albums: 0,
        total_images: 0,
        duplicate_count: 0,
        error_count: 0,
        errors: Vec::new(),
    };
    let mut errors: Vec<String> = Vec::new();
    let mut processed_images: u32 = 0;
    let mut discovered_images: u32 = 0;
    let mut duplicate_count: u32 = 0;

    let source_path = PathBuf::from(&source_root);

    let (client, handle) = {
        let mgr = postgres_manager.lock().await;
        mgr.connect().await?
    };
    DatabaseOperationLock::acquire_shared(&client, "import scan").await?;

    let library_root_id = ImportRepository::upsert_default_library_root(&client).await?;
    if let Some(library_root) = {
        let settings = settings.lock().await;
        settings.get().library_root.clone()
    } {
        ImportRepository::update_library_root_path(&client, library_root_id, &library_root).await?;
    }

    let existing_run = if let Some(id) = resume_import_run_id {
        let row = client
            .query_opt(
                "SELECT id FROM import_runs
                 WHERE id = $1
                   AND source_root = $2
                   AND state IN ('analyzing', 'scanning', 'fingerprinting', 'cancelled', 'failed')
                 LIMIT 1",
                &[&id, &source_root],
            )
            .await
            .map_err(|e| {
                AppError::Internal(format!(
                    "failed to query requested resumable import run: {e}"
                ))
            })?
            .ok_or_else(|| {
                AppError::Internal(format!(
                    "import run {id} is not resumable for source root {source_root}"
                ))
            })?;
        Some(row)
    } else {
        // An ordinary start is an explicit request for a clean analysis. Only
        // resume_import_run may reuse immutable snapshots from an older run.
        None
    };
    let import_run_id = if let Some(row) = existing_run {
        let id: Uuid = row.get("id");
        ImportRepository::mark_stale_analyzing_albums(&client, id).await?;
        ImportRepository::reopen_import_run_for_analysis(&client, id).await?;
        id
    } else {
        ImportRepository::create_import_run(&client, &source_root, library_root_id).await?
    };
    progress.import_run_id = Some(import_run_id.to_string());
    tracing::info!(
        %import_run_id,
        %library_root_id,
        source_root = %source_path.display(),
        "scan import run created"
    );

    emit_progress(&progress, &progress_tracker).await;

    let scan_root = source_path.clone();
    let root_cancel = cancelled.clone();
    let albums = match tokio::task::spawn_blocking(move || {
        scan_directory_for_albums_with_cancel(&scan_root, Some(&root_cancel))
    })
    .await
    .map_err(|e| AppError::Internal(format!("source album enumeration worker failed: {e}")))?
    {
        Ok(albums) => albums,
        Err(e) => {
            ImportRepository::update_import_run_error(
                &client,
                import_run_id,
                "SCAN_FAILED",
                &e.to_string(),
            )
            .await?;
            ImportRepository::refresh_import_run_statistics(&client, import_run_id).await?;
            tracing::error!(
                %import_run_id,
                error = %e,
                "scan failed while enumerating source albums"
            );
            handle.abort();
            return Err(e);
        }
    };

    progress.total_albums = albums.len() as u32;
    emit_progress(&progress, &progress_tracker).await;
    tracing::info!(
        %import_run_id,
        total_albums = progress.total_albums,
        "scan source albums enumerated"
    );

    let stored_album_count: i64 = client
        .query_one(
            "SELECT COUNT(*) FROM import_albums WHERE import_run_id = $1",
            &[&import_run_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to count stored albums: {e}")))?
        .get(0);
    if albums.is_empty() && stored_album_count == 0 {
        ImportRepository::update_import_run_state(
            &client,
            import_run_id,
            &ImportRunState::Completed,
        )
        .await?;
        progress.state = "completed".to_string();
        progress.current_stage = "completed".to_string();
        emit_progress(&progress, &progress_tracker).await;
        tracing::info!(%import_run_id, elapsed_ms = started_at.elapsed().as_millis(), "scan completed with no albums");
        handle.abort();
        return Ok(ScanProgress::idle());
    }

    for album in &albums {
        ImportRepository::insert_import_album(
            &client,
            import_run_id,
            &album.path.display().to_string(),
            &album.name,
        )
        .await?;
    }

    ImportRepository::mark_stale_analyzing_albums(&client, import_run_id).await?;
    ImportRepository::update_import_run_state(&client, import_run_id, &ImportRunState::Analyzing)
        .await?;
    progress.current_stage = "analyzing".to_string();
    emit_progress(&progress, &progress_tracker).await;
    tracing::info!(%import_run_id, "album workflow analysis stage started");

    let analyzed_exact_rows =
        ImportRepository::get_analyzed_run_exact_representatives(&client, import_run_id).await?;
    let mut run_exact_index = RunExactRepresentativeIndex::from_rows(analyzed_exact_rows);

    let album_rows = client
        .query(
            "SELECT id, source_path, source_name, state
             FROM import_albums
             WHERE import_run_id = $1
               AND state IN ('pending', 'analyzing', 'scanning', 'fingerprinting')
             ORDER BY source_name",
            &[&import_run_id],
        )
        .await
        .map_err(|e| AppError::Internal(format!("failed to query albums: {e}")))?;

    // Establish the denominator before any fingerprint work starts. This is
    // deliberately a read-only pre-count; each album is enumerated again only
    // after its immutable snapshot is captured and verified.
    for album_row in &album_rows {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let album_path = PathBuf::from(album_row.get::<_, String>("source_path"));
        let album_name: String = album_row.get("source_name");
        let count_cancel = cancelled.clone();
        match tokio::task::spawn_blocking(move || {
            scan_album_for_images_with_cancel(&album_path, &album_name, Some(&count_cancel))
                .map(|images| images.len() as u32)
        })
        .await
        .map_err(|e| AppError::Internal(format!("image pre-count worker failed: {e}")))?
        {
            Ok(count) => discovered_images += count,
            Err(_) if cancelled.load(Ordering::Relaxed) => break,
            // The authoritative album pass records a per-album failure with
            // context; a pre-count failure must not fail the whole run early.
            Err(_) => {}
        }
    }
    progress.total_images = discovered_images;
    emit_progress(&progress, &progress_tracker).await;

    for album_row in &album_rows {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }

        let album_db_id: Uuid = album_row.get("id");
        let album_source_path: String = album_row.get("source_path");
        let album_name: String = album_row.get("source_name");

        ImportRepository::mark_import_album_analyzing(&client, album_db_id).await?;

        progress.current_album = Some(album_name.clone());
        progress.current_stage = "analyzing".to_string();
        emit_progress(&progress, &progress_tracker).await;
        tracing::info!(
            %import_run_id,
            %album_db_id,
            album = %album_name,
            "scan album started"
        );

        let album_path = PathBuf::from(&album_source_path);
        if let Err(e) = capture_source_album_snapshot_with_cancel(
            &client,
            import_run_id,
            album_db_id,
            &album_path,
            Some(cancelled.clone()),
        )
        .await
        {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            let msg = format!("Failed to snapshot album '{}': {e}", album_name);
            errors.push(msg.clone());
            progress.error_count = errors.len() as u32;
            progress.errors = errors.clone();
            ImportRepository::mark_import_album_failed(
                &client,
                album_db_id,
                "SNAPSHOT_FAILED",
                &msg,
            )
            .await?;
            emit_progress(&progress, &progress_tracker).await;
            continue;
        }
        match verify_source_album_snapshot_with_cancel(
            &client,
            album_db_id,
            &album_path,
            Some(cancelled.clone()),
        )
        .await
        {
            Ok(snapshot_errors) if snapshot_errors.is_empty() => {}
            Ok(snapshot_errors) => {
                let msg = format!(
                    "Source snapshot verification failed for '{}': {}",
                    album_name,
                    snapshot_errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                );
                errors.push(msg.clone());
                progress.error_count = errors.len() as u32;
                progress.errors = errors.clone();
                ImportRepository::mark_import_album_failed(
                    &client,
                    album_db_id,
                    "SNAPSHOT_VERIFY_FAILED",
                    &msg,
                )
                .await?;
                emit_progress(&progress, &progress_tracker).await;
                continue;
            }
            Err(e) => {
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }
                let msg = format!("Failed to verify snapshot for '{}': {e}", album_name);
                errors.push(msg.clone());
                progress.error_count = errors.len() as u32;
                progress.errors = errors.clone();
                ImportRepository::mark_import_album_failed(
                    &client,
                    album_db_id,
                    "SNAPSHOT_VERIFY_FAILED",
                    &msg,
                )
                .await?;
                emit_progress(&progress, &progress_tracker).await;
                continue;
            }
        }

        progress.current_stage = "fingerprinting".to_string();
        emit_progress(&progress, &progress_tracker).await;
        let scan_album_path = album_path.clone();
        let scan_album_name = album_name.clone();
        let album_cancel = cancelled.clone();
        let scanned_images = match tokio::task::spawn_blocking(move || {
            scan_album_for_images_with_cancel(
                &scan_album_path,
                &scan_album_name,
                Some(&album_cancel),
            )
        })
        .await
        .map_err(|e| AppError::Internal(format!("album enumeration worker failed: {e}")))?
        {
            Ok(imgs) => imgs,
            Err(e) => {
                let msg = format!("Failed to scan album '{}': {e}", album_name);
                tracing::warn!(
                    %import_run_id,
                    %album_db_id,
                    album = %album_name,
                    error = %e,
                    "scan album enumeration failed"
                );
                errors.push(msg.clone());
                progress.error_count = errors.len() as u32;
                progress.errors = errors.clone();
                ImportRepository::mark_import_album_failed(
                    &client,
                    album_db_id,
                    "SCAN_ALBUM_FAILED",
                    &msg,
                )
                .await?;
                emit_progress(&progress, &progress_tracker).await;
                continue;
            }
        };

        let mut album_metrics = AlbumAnalysisMetrics {
            image_count: scanned_images.len(),
            ..AlbumAnalysisMetrics::default()
        };
        let fingerprint_started = Instant::now();
        let mut fingerprint_jobs = tokio::task::JoinSet::new();
        for img in scanned_images {
            fingerprint_jobs.spawn(fingerprint_scanned_image(img));
        }

        let mut image_batch: Vec<(Uuid, NewImportImage)> = Vec::new();
        let mut fingerprinted_batch: Vec<(Uuid, String, FingerprintedData)> = Vec::new();
        while !fingerprint_jobs.is_empty() {
            let joined = tokio::select! {
                result = fingerprint_jobs.join_next() => result,
                _ = tokio::time::sleep(std::time::Duration::from_millis(25)) => {
                    if cancelled.load(Ordering::Relaxed) {
                        fingerprint_jobs.abort_all();
                        break;
                    }
                    continue;
                }
            };
            let Some(joined) = joined else { break };
            let (img, fp_result) = joined
                .map_err(|e| AppError::Internal(format!("fingerprint task failed: {e}")))??;

            match fp_result {
                Ok(fp) => {
                    let image_id = Uuid::new_v4();
                    image_batch.push((
                        image_id,
                        NewImportImage {
                            album_id: album_db_id,
                            source_path: img.source_path.display().to_string(),
                            relative_path: img.relative_path.clone(),
                            file_size: img.file_size,
                            modified_at: img.modified_at,
                            width: Some(fp.width as i32),
                            height: Some(fp.height as i32),
                            format: Some(fp.format.clone()),
                            decode_state: DecodeState::Decoded,
                            blake3: Some(fp.blake3_bytes.clone()),
                            pixel_hash: Some(fp.pixel_hash_bytes.clone()),
                            block_hash_16: Some(fp.block_hash_16.clone()),
                            double_gradient_hash_32: Some(fp.double_gradient_hash_32.clone()),
                            perceptual_eligible: fp.perceptual_eligible,
                            fingerprint_version: Some(FINGERPRINT_VERSION.to_string()),
                            state: ImportImageState::Fingerprinted,
                        },
                    ));
                    fingerprinted_batch.push((image_id, img.relative_path.clone(), fp));
                    album_metrics.fingerprint_success_count += 1;

                    processed_images += 1;
                    progress.processed_images = processed_images;
                    emit_progress(&progress, &progress_tracker).await;
                }
                Err(e) => {
                    image_batch.push((
                        Uuid::new_v4(),
                        NewImportImage {
                            album_id: album_db_id,
                            source_path: img.source_path.display().to_string(),
                            relative_path: img.relative_path.clone(),
                            file_size: img.file_size,
                            modified_at: img.modified_at,
                            width: None,
                            height: None,
                            format: None,
                            decode_state: DecodeState::Failed,
                            blake3: None,
                            pixel_hash: None,
                            block_hash_16: None,
                            double_gradient_hash_32: None,
                            perceptual_eligible: false,
                            fingerprint_version: None,
                            state: ImportImageState::Failed,
                        },
                    ));

                    let msg = format!("Failed to fingerprint '{}': {e}", img.source_path.display());
                    tracing::warn!(
                        %import_run_id,
                        %album_db_id,
                        album = %album_name,
                        source_path = %img.source_path.display(),
                        error = %e,
                        "scan image fingerprint failed"
                    );
                    errors.push(msg.clone());
                    progress.error_count = errors.len() as u32;
                    progress.errors = errors.clone();
                    processed_images += 1;
                    progress.processed_images = processed_images;
                    emit_progress(&progress, &progress_tracker).await;
                    album_metrics.fingerprint_failure_count += 1;
                }
            }
        }
        album_metrics.fingerprint_ms = fingerprint_started.elapsed().as_millis();

        ImportRepository::insert_import_images_batch(&client, &image_batch).await?;
        fingerprinted_batch.sort_by(|left, right| left.1.cmp(&right.1));
        let album_images: Vec<AlbumImageEntry> = fingerprinted_batch
            .into_iter()
            .map(|(image_db_id, _, fp)| AlbumImageEntry { image_db_id, fp })
            .collect();

        if cancelled.load(Ordering::Relaxed) {
            break;
        }

        progress.current_stage = "detecting_duplicates".to_string();
        emit_progress(&progress, &progress_tracker).await;
        let album_image_refs: Vec<&AlbumImageEntry> = album_images.iter().collect();
        let detection_ctx = AlbumDetectionContext {
            client: &client,
            import_run_id,
            album_id: album_db_id,
            cancelled: &cancelled,
            library_index: &library_index,
            run_exact_index: &run_exact_index,
        };
        if let Err(e) = detect_album_duplicates(
            &detection_ctx,
            &album_image_refs,
            &mut duplicate_count,
            &mut progress,
            &mut album_metrics,
        )
        .await
        {
            let msg = format!(
                "Failed to detect duplicates for album '{}': {e}",
                album_name
            );
            errors.push(msg.clone());
            progress.error_count = errors.len() as u32;
            progress.errors = errors.clone();
            ImportRepository::mark_import_album_failed(
                &client,
                album_db_id,
                "DUPLICATE_DETECTION_FAILED",
                &msg,
            )
            .await?;
            emit_progress(&progress, &progress_tracker).await;
            continue;
        }

        if cancelled.load(Ordering::Relaxed) {
            // detect_album_duplicates stops cooperatively and returns Ok so
            // cancellation is not reported as an analysis failure. Do not
            // turn that partial candidate set into a completed checkpoint;
            // keep the album analyzing so resume can clean and rerun it.
            break;
        }

        let album_status =
            ImportRepository::finalize_import_album_analysis(&client, album_db_id).await?;
        run_exact_index.add_album(album_db_id, &album_images);
        emit_progress(&progress, &progress_tracker).await;
        tracing::info!(
            %import_run_id,
            %album_db_id,
            album = %album_name,
            processed_images = progress.processed_images,
            album_state = %album_status.state,
            review_candidates = album_status.review_candidate_count,
            error_count = progress.error_count,
            image_count = album_metrics.image_count,
            fingerprint_success_count = album_metrics.fingerprint_success_count,
            fingerprint_failure_count = album_metrics.fingerprint_failure_count,
            perceptual_ineligible_count = album_metrics.perceptual_ineligible_count,
            file_exact_candidate_count = album_metrics.file_exact_candidate_count,
            pixel_exact_candidate_count = album_metrics.pixel_exact_candidate_count,
            perceptual_recall_candidate_count = album_metrics.perceptual_recall_candidate_count,
            perceptual_auto_duplicate_count = album_metrics.perceptual_auto_duplicate_count,
            review_candidate_count = album_metrics.review_candidate_count,
            truncated_image_count = album_metrics.truncated_image_count,
            fingerprint_ms = album_metrics.fingerprint_ms,
            intra_detection_ms = album_metrics.intra_detection_ms,
            library_recall_ms = album_metrics.library_recall_ms,
            fine_verification_ms = album_metrics.fine_verification_ms,
            database_write_ms = album_metrics.database_write_ms,
            "scan album analysis checkpoint persisted"
        );
    }

    // Group membership is frozen only after every album has reached a complete
    // analysis state. A late cancellation after the last checkpoint is safe:
    // the same persisted facts produce the complete group set before the run
    // state is reconciled.
    let incomplete_album_count: i64 = client
        .query_one(
            "SELECT COUNT(*)::BIGINT FROM import_albums
             WHERE import_run_id = $1
               AND state NOT IN ('analyzed', 'review_required')",
            &[&import_run_id],
        )
        .await
        .map_err(|e| {
            AppError::Internal(format!(
                "failed to check group materialization readiness: {e}"
            ))
        })?
        .get(0);
    if incomplete_album_count == 0 {
        crate::services::review_service::materialize_review_groups(&client, import_run_id).await?;
    }

    if cancelled.load(Ordering::Relaxed) {
        ImportRepository::refresh_import_run_statistics(&client, import_run_id).await?;
        let final_state =
            ImportRepository::reconcile_scan_run_state_after_cancellation(&client, import_run_id)
                .await?;
        let final_state_str = final_state.to_string();
        progress.state = final_state_str.clone();
        progress.current_stage = final_state_str.clone();
        emit_progress(&progress, &progress_tracker).await;
        tracing::warn!(
            %import_run_id,
            processed_images = progress.processed_images,
            total_albums = progress.total_albums,
            elapsed_ms = started_at.elapsed().as_millis(),
            "scan cancelled"
        );
        handle.abort();
        return Ok(ScanProgress {
            state: final_state_str,
            ..ScanProgress::idle()
        });
    }

    ImportRepository::refresh_import_run_statistics(&client, import_run_id).await?;

    // Determine post-scan state: if any undecided duplicate candidates remain,
    // the run needs review; otherwise it is ready to commit. The run is NOT
    // marked Completed here — completion happens after commit + archive.
    let review_progress = ImportRepository::get_review_progress(&client, import_run_id).await?;
    let run_summary = ImportRepository::list_import_runs_summary(&client)
        .await?
        .into_iter()
        .find(|summary| summary.import_run_id == import_run_id.to_string());
    let has_failed_albums = run_summary
        .as_ref()
        .map(|summary| summary.failed_albums > 0)
        .unwrap_or(false);
    let has_unfinished_albums = run_summary
        .as_ref()
        .map(|summary| summary.pending_albums > 0 || summary.analyzing_albums > 0)
        .unwrap_or(false);
    let final_run_state = if has_failed_albums || has_unfinished_albums {
        ImportRunState::Failed
    } else if review_progress.total > review_progress.decided {
        ImportRunState::ReviewRequired
    } else {
        ImportRunState::ReadyToCommit
    };
    ImportRepository::update_import_run_state(&client, import_run_id, &final_run_state).await?;

    progress.state = final_run_state.to_string();
    progress.current_stage = final_run_state.to_string();
    progress.current_album = None;
    emit_progress(&progress, &progress_tracker).await;
    tracing::info!(
        %import_run_id,
        final_state = %final_run_state,
        total_albums = progress.total_albums,
        total_images = discovered_images,
        duplicate_count,
        error_count = errors.len(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "scan finished"
    );

    handle.abort();

    Ok(ScanProgress {
        state: final_run_state.to_string(),
        import_run_id: Some(import_run_id.to_string()),
        current_stage: final_run_state.to_string(),
        current_album: None,
        processed_images,
        total_albums: progress.total_albums,
        total_images: discovered_images,
        duplicate_count,
        error_count: errors.len() as u32,
        errors,
    })
}

pub async fn scan_source_info(
    source_root: &str,
) -> Result<crate::domain::import_state::ScanSourceInfo, AppError> {
    let p = Path::new(source_root);
    if !p.exists() {
        return Err(AppError::Internal(format!(
            "directory does not exist: {source_root}"
        )));
    }
    if !p.is_dir() {
        return Err(AppError::Internal(format!(
            "not a directory: {source_root}"
        )));
    }
    let albums = scan_directory_for_albums(p)?;
    let names: Vec<String> = albums.into_iter().map(|a| a.name).collect();
    let count = names.len() as u32;
    Ok(crate::domain::import_state::ScanSourceInfo {
        path: source_root.to_string(),
        albums: names,
        album_count: count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "real-db-tests")]
    use crate::domain::import_state::ImportAlbumState;
    use crate::repositories::import_repository::NewSnapshotFile;
    #[cfg(feature = "real-db-tests")]
    use crate::services::source_snapshot_service::SnapshotVerifyError;
    #[cfg(feature = "real-db-tests")]
    use crate::services::source_snapshot_service::{
        capture_source_album_snapshot, verify_source_album_snapshot,
    };
    use crate::services::source_snapshot_service::{collect_album_files, compute_snapshot_hash};
    use tempfile::TempDir;

    fn create_test_album(tmp: &Path, album_name: &str) -> PathBuf {
        let album_dir = tmp.join(album_name);
        std::fs::create_dir_all(&album_dir).unwrap();
        album_dir
    }

    fn create_test_image(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let img = image::RgbImage::new(16, 16);
        img.save(&path).unwrap();
        path
    }

    fn synthetic_fingerprint(marker: u8) -> FingerprintedData {
        FingerprintedData {
            file_size: 1,
            width: 32,
            height: 32,
            format: "png".to_string(),
            blake3_bytes: vec![marker; 32],
            pixel_hash_bytes: vec![marker; 32],
            block_hash_16: vec![marker; 32],
            double_gradient_hash_32: vec![marker; 68],
            perceptual_eligible: true,
            block_variants: TransformType::ALL
                .iter()
                .map(|&transform| BlockHashVariant {
                    transform,
                    hash: vec![marker; 32],
                })
                .collect(),
            fine_thumbnail_32: image::GrayImage::new(32, 32),
        }
    }

    #[test]
    fn exact_duplicate_groups_use_one_stable_representative_edge_per_member() {
        let import_run_id = Uuid::from_u128(999);
        let album_id = Uuid::from_u128(1000);
        let images: Vec<_> = (1..=20_u128)
            .rev()
            .map(|id| AlbumImageEntry {
                image_db_id: Uuid::from_u128(id),
                fp: synthetic_fingerprint(7),
            })
            .collect();
        let refs: Vec<_> = images.iter().collect();
        let mut candidates = HashMap::new();
        let run_index = RunExactRepresentativeIndex::default();

        insert_run_exact_representative_candidates(
            import_run_id,
            album_id,
            &refs,
            &run_index,
            &mut candidates,
        )
        .unwrap();

        let representative = Uuid::from_u128(1);
        assert_eq!(candidates.len(), 19);
        assert!(candidates.values().all(|candidate| {
            candidate.match_type == MatchType::FileExact
                && candidate.source_image_id == representative
                && candidate.candidate_source_image_id != Some(representative)
        }));
    }

    #[test]
    fn run_exact_groups_across_four_albums_store_only_nineteen_edges() {
        let import_run_id = Uuid::from_u128(900);
        let mut run_index = RunExactRepresentativeIndex::default();
        let mut candidates = HashMap::new();

        for album_number in 0..4_u128 {
            let album_id = Uuid::from_u128(100 + album_number);
            let images: Vec<_> = (1..=5_u128)
                .map(|offset| AlbumImageEntry {
                    image_db_id: Uuid::from_u128(album_number * 5 + offset),
                    fp: synthetic_fingerprint(7),
                })
                .collect();
            let refs: Vec<_> = images.iter().collect();
            insert_run_exact_representative_candidates(
                import_run_id,
                album_id,
                &refs,
                &run_index,
                &mut candidates,
            )
            .unwrap();
            run_index.add_album(album_id, &images);
        }

        assert_eq!(candidates.len(), 19);
        assert_eq!(
            candidates
                .values()
                .filter(|candidate| candidate.scope == DuplicateScope::CrossAlbum)
                .count(),
            15
        );
        assert!(candidates
            .values()
            .all(|candidate| candidate.match_type == MatchType::FileExact));
    }

    #[test]
    fn tied_block_transforms_are_ranked_by_double_gradient_distance() {
        let thumbnail = image::GrayImage::from_fn(32, 32, |x, y| {
            image::Luma([((x * 11 + y * 17 + (x * y) % 29) % 251) as u8])
        });
        let rot90_fine = compute_double_gradient_for_transform(&thumbnail, TransformType::Rot90);
        let flip_h_fine = compute_double_gradient_for_transform(&thumbnail, TransformType::FlipH);
        assert_ne!(
            rot90_fine, flip_h_fine,
            "fixture must distinguish transforms"
        );
        let mut source = synthetic_fingerprint(0);
        source.fine_thumbnail_32 = thumbnail;
        source.block_variants = vec![
            BlockHashVariant {
                transform: TransformType::FlipH,
                hash: vec![0; 32],
            },
            BlockHashVariant {
                transform: TransformType::Rot90,
                hash: vec![0; 32],
            },
        ];

        let evidence = select_best_fine_transform(
            &source,
            true,
            &[0; 32],
            &rot90_fine,
            &[TransformType::FlipH, TransformType::Rot90],
            &mut HashMap::new(),
        )
        .unwrap();

        assert_eq!(evidence.block_distance, 0);
        assert_eq!(evidence.double_gradient_distance, 0);
        assert_eq!(evidence.transform_type, TransformType::Rot90);
    }

    fn perceptual_evidence_between(
        source: &FingerprintedData,
        candidate: &FingerprintedData,
    ) -> PerceptualEvidence {
        let (variant, block_distance) = source
            .block_variants
            .iter()
            .map(|variant| {
                let distance = hamming_distance(&variant.hash, &candidate.block_hash_16).unwrap();
                (variant, distance)
            })
            .min_by(
                |(left_variant, left_distance), (right_variant, right_distance)| {
                    left_distance
                        .raw_distance
                        .cmp(&right_distance.raw_distance)
                        .then_with(|| {
                            left_variant
                                .transform
                                .to_string()
                                .cmp(&right_variant.transform.to_string())
                        })
                },
            )
            .unwrap();
        let source_fine =
            compute_double_gradient_for_transform(&source.fine_thumbnail_32, variant.transform);
        let fine_distance =
            hamming_distance(&source_fine, &candidate.double_gradient_hash_32).unwrap();
        PerceptualEvidence {
            perceptual_eligible: source.perceptual_eligible && candidate.perceptual_eligible,
            block_distance: block_distance.raw_distance as i32,
            double_gradient_distance: fine_distance.raw_distance as i32,
            block_distance_ratio: block_distance.normalized_distance,
            double_gradient_distance_ratio: fine_distance.normalized_distance,
            transform_type: variant.transform,
            confidence: weighted_similarity(
                block_distance.normalized_distance,
                fine_distance.normalized_distance,
            ),
        }
    }

    fn patterned_scene(subject_x: u32, subject_pose: u32) -> image::RgbImage {
        let mut image = image::RgbImage::from_fn(192, 128, |x, y| {
            let base = ((x * 3 + y * 5 + (x * y) % 97) % 180) as u8;
            image::Rgb([base, base.saturating_add(24), 220u8.saturating_sub(base)])
        });
        for y in 35..95 {
            for x in subject_x..(subject_x + 45) {
                let edge = x == subject_x || x == subject_x + 44 || y == 35 || y == 94;
                let pose_mark = (x + y + subject_pose) % 13 < 4;
                image.put_pixel(
                    x,
                    y,
                    if edge || pose_mark {
                        image::Rgb([245, 235, 36])
                    } else {
                        image::Rgb([34, 52, 205])
                    },
                );
            }
        }
        image
    }

    fn save_jpeg_with_quality(path: &Path, image: &image::RgbImage, quality: u8) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality);
        encoder.encode_image(image).unwrap();
    }

    #[cfg(feature = "real-db-tests")]
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in bytes {
            crc ^= *byte as u32;
            for _ in 0..8 {
                let mask = if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
                crc = (crc >> 1) ^ mask;
            }
        }
        !crc
    }

    #[cfg(feature = "real-db-tests")]
    fn write_png_with_text_chunk(source: &Path, target: &Path) {
        let bytes = std::fs::read(source).unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));

        let mut iend_pos = None;
        let mut offset = 8usize;
        while offset + 12 <= bytes.len() {
            let len = u32::from_be_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]) as usize;
            let kind_start = offset + 4;
            let data_start = offset + 8;
            let next = data_start + len + 4;
            assert!(next <= bytes.len(), "invalid png chunk length");
            if &bytes[kind_start..kind_start + 4] == b"IEND" {
                iend_pos = Some(offset);
                break;
            }
            offset = next;
        }

        let iend_pos = iend_pos.expect("IEND chunk");
        let chunk_type = b"tEXt";
        let chunk_data = b"Comment\0ImageDB metadata variant";
        let mut crc_input = Vec::new();
        crc_input.extend_from_slice(chunk_type);
        crc_input.extend_from_slice(chunk_data);
        let crc = crc32(&crc_input);

        let mut text_chunk = Vec::new();
        text_chunk.extend_from_slice(&(chunk_data.len() as u32).to_be_bytes());
        text_chunk.extend_from_slice(chunk_type);
        text_chunk.extend_from_slice(chunk_data);
        text_chunk.extend_from_slice(&crc.to_be_bytes());

        let mut out = Vec::new();
        out.extend_from_slice(&bytes[..iend_pos]);
        out.extend_from_slice(&text_chunk);
        out.extend_from_slice(&bytes[iend_pos..]);
        std::fs::write(target, out).unwrap();
    }

    #[test]
    fn test_scan_directory_for_albums() {
        let tmp = TempDir::new().unwrap();
        create_test_album(tmp.path(), "album_a");
        create_test_album(tmp.path(), "album_b");
        std::fs::write(tmp.path().join("readme.txt"), "not an album").unwrap();

        let albums = scan_directory_for_albums(tmp.path()).unwrap();
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0].name, "album_a");
        assert_eq!(albums[1].name, "album_b");
    }

    #[test]
    fn test_scan_album_for_images() {
        let tmp = TempDir::new().unwrap();
        let album = create_test_album(tmp.path(), "vacation");
        create_test_image(&album, "photo1.jpg");
        create_test_image(&album, "photo2.png");
        create_test_image(&album, "photo3.webp");
        std::fs::write(album.join("notes.txt"), "ignored").unwrap();
        std::fs::write(album.join("data.bmp"), "ignored").unwrap();

        let images = scan_album_for_images(&album, "vacation").unwrap();
        assert_eq!(images.len(), 3);
        for img in &images {
            assert!(
                img.relative_path.starts_with("vacation/"),
                "unexpected relative path: {}",
                img.relative_path
            );
            assert!(img.file_size > 0);
        }
    }

    #[test]
    fn test_scan_album_for_images_recurses_into_subdirectories() {
        let tmp = TempDir::new().unwrap();
        let album = create_test_album(tmp.path(), "vacation");
        create_test_image(&album, "cover.jpg");
        let chapter = album.join("chapter_1");
        let extras = chapter.join("extras");
        std::fs::create_dir_all(&extras).unwrap();
        create_test_image(&chapter, "page01.png");
        create_test_image(&extras, "detail.webp");
        std::fs::write(extras.join("notes.txt"), "ignored").unwrap();

        let images = scan_album_for_images(&album, "vacation").unwrap();
        let paths: Vec<&str> = images
            .iter()
            .map(|image| image.relative_path.as_str())
            .collect();

        assert_eq!(paths.len(), 3);
        assert_eq!(
            paths,
            vec![
                "vacation/chapter_1/extras/detail.webp",
                "vacation/chapter_1/page01.png",
                "vacation/cover.jpg",
            ]
        );
    }

    #[test]
    fn test_scan_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let albums = scan_directory_for_albums(tmp.path()).unwrap();
        assert!(albums.is_empty());
    }

    #[test]
    fn test_scan_nonexistent_directory() {
        let result = scan_directory_for_albums(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_fingerprint_image_sync() {
        let tmp = TempDir::new().unwrap();
        let path = create_test_image(tmp.path(), "test.png");
        let fp = fingerprint_image_sync(&path).unwrap();
        assert!(fp.width > 0);
        assert!(fp.height > 0);
        assert_eq!(fp.blake3_bytes.len(), 32);
        assert_eq!(fp.pixel_hash_bytes.len(), 32);
        assert_eq!(fp.block_hash_16.len(), 32);
        assert_eq!(fp.double_gradient_hash_32.len(), 68);
        assert_eq!(fp.block_variants.len(), 8);
    }

    #[test]
    fn large_image_decode_policy_is_single_slot_at_the_product_threshold() {
        assert!(!requires_serial_large_image_decode(
            LARGE_IMAGE_PIXEL_THRESHOLD - 1
        ));
        assert!(requires_serial_large_image_decode(
            LARGE_IMAGE_PIXEL_THRESHOLD
        ));
        assert_eq!(large_image_decode_limit().available_permits(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fingerprint_worker_keeps_async_runtime_responsive() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("large.png");
        let image = image::RgbImage::from_fn(3000, 3000, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8])
        });
        image.save(&path).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let scanned = ScannedImage {
            source_path: path,
            relative_path: "album/large.png".to_string(),
            file_size: metadata.len() as i64,
            modified_at: None,
        };

        let mut job = tokio::spawn(fingerprint_scanned_image(scanned));
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
            _ = &mut job => panic!("fingerprint unexpectedly completed before runtime responsiveness probe"),
        }
        let (_, fingerprint) = job.await.unwrap().unwrap();
        assert!(fingerprint.is_ok());
    }

    #[test]
    fn test_duplicate_detection_file_exact() {
        let tmp = TempDir::new().unwrap();
        let album = create_test_album(tmp.path(), "dup_album");
        let original = create_test_image(&album, "original.png");
        let copy = album.join("copy.png");
        std::fs::copy(&original, &copy).unwrap();

        let fp1 = fingerprint_image_sync(&original).unwrap();
        let fp2 = fingerprint_image_sync(&copy).unwrap();

        assert_eq!(
            fp1.blake3_bytes, fp2.blake3_bytes,
            "BLAKE3 should match for exact copy"
        );
        assert_eq!(
            fp1.pixel_hash_bytes, fp2.pixel_hash_bytes,
            "pixel hash should match for exact copy"
        );
    }

    #[test]
    fn test_duplicate_detection_renamed_file() {
        let tmp = TempDir::new().unwrap();
        let album = create_test_album(tmp.path(), "rename_album");
        let original = create_test_image(&album, "photo.png");
        let renamed = album.join("renamed_photo.png");
        std::fs::copy(&original, &renamed).unwrap();

        let fp1 = fingerprint_image_sync(&original).unwrap();
        let fp2 = fingerprint_image_sync(&renamed).unwrap();

        assert_eq!(
            fp1.blake3_bytes, fp2.blake3_bytes,
            "renamed file should have same BLAKE3"
        );
    }

    #[test]
    fn test_duplicate_detection_pixel_identical_different_format() {
        let tmp = TempDir::new().unwrap();
        let album = create_test_album(tmp.path(), "format_album");

        let img = image::RgbImage::new(32, 32);
        let png_path = album.join("image.png");
        let jpg_path = album.join("image.jpg");
        img.save(&png_path).unwrap();
        img.save(&jpg_path).unwrap();

        let fp_png = fingerprint_image_sync(&png_path).unwrap();
        let fp_jpg = fingerprint_image_sync(&jpg_path).unwrap();

        assert_ne!(
            fp_png.blake3_bytes, fp_jpg.blake3_bytes,
            "different formats should have different BLAKE3"
        );
    }

    #[test]
    fn v2_perceptual_regression_small_generated_variants() {
        let tmp = TempDir::new().unwrap();
        let base = patterned_scene(36, 0);
        let base_path = tmp.path().join("base.png");
        base.save(&base_path).unwrap();
        let base_fp = fingerprint_image_sync(&base_path).unwrap();

        let resized_path = tmp.path().join("resized.png");
        image::imageops::resize(&base, 96, 64, image::imageops::FilterType::Triangle)
            .save(&resized_path)
            .unwrap();
        let resized = fingerprint_image_sync(&resized_path).unwrap();
        let resized_evidence = perceptual_evidence_between(&resized, &base_fp);
        assert_eq!(
            classify_perceptual(&resized_evidence).and_then(|(_, decision, _)| decision),
            Some(Decision::AutoDuplicate),
            "resized copy should auto-match: {resized_evidence:?}"
        );

        let jpeg_high_path = tmp.path().join("quality-high.jpg");
        let jpeg_low_path = tmp.path().join("quality-low.jpg");
        save_jpeg_with_quality(&jpeg_high_path, &base, 92);
        save_jpeg_with_quality(&jpeg_low_path, &base, 68);
        let jpeg_high = fingerprint_image_sync(&jpeg_high_path).unwrap();
        let jpeg_low = fingerprint_image_sync(&jpeg_low_path).unwrap();
        let jpeg_evidence = perceptual_evidence_between(&jpeg_low, &jpeg_high);
        assert!(
            classify_perceptual(&jpeg_evidence).is_some(),
            "JPEG quality variants should remain reviewable: {jpeg_evidence:?}"
        );

        let mut brighter = base.clone();
        for pixel in brighter.pixels_mut() {
            for channel in &mut pixel.0 {
                *channel = channel.saturating_add(10);
            }
        }
        let brighter_path = tmp.path().join("brighter.png");
        brighter.save(&brighter_path).unwrap();
        let brighter_fp = fingerprint_image_sync(&brighter_path).unwrap();
        let brightness_evidence = perceptual_evidence_between(&brighter_fp, &base_fp);
        assert!(
            classify_perceptual(&brightness_evidence).is_some(),
            "small brightness change should remain reviewable: {brightness_evidence:?}"
        );

        let mut watermarked = base.clone();
        for y in 106..118 {
            for x in 168..184 {
                watermarked.put_pixel(x, y, image::Rgb([250, 250, 250]));
            }
        }
        let watermarked_path = tmp.path().join("watermarked.png");
        watermarked.save(&watermarked_path).unwrap();
        let watermarked_fp = fingerprint_image_sync(&watermarked_path).unwrap();
        let watermark_evidence = perceptual_evidence_between(&watermarked_fp, &base_fp);
        assert!(
            classify_perceptual(&watermark_evidence).is_some(),
            "small watermark should remain reviewable: {watermark_evidence:?}"
        );

        let shifted_path = tmp.path().join("shifted-subject.png");
        patterned_scene(92, 0).save(&shifted_path).unwrap();
        let shifted = fingerprint_image_sync(&shifted_path).unwrap();
        let shifted_evidence = perceptual_evidence_between(&shifted, &base_fp);
        assert_ne!(
            classify_perceptual(&shifted_evidence).and_then(|(_, decision, _)| decision),
            Some(Decision::AutoDuplicate),
            "subject position change must not auto-match: {shifted_evidence:?}"
        );

        let adjacent_path = tmp.path().join("adjacent-action.png");
        patterned_scene(50, 8).save(&adjacent_path).unwrap();
        let adjacent = fingerprint_image_sync(&adjacent_path).unwrap();
        let adjacent_evidence = perceptual_evidence_between(&adjacent, &base_fp);
        assert_ne!(
            classify_perceptual(&adjacent_evidence).and_then(|(_, decision, _)| decision),
            Some(Decision::AutoDuplicate),
            "adjacent action must not auto-match: {adjacent_evidence:?}"
        );

        let different = image::RgbImage::from_fn(192, 128, |x, y| {
            if (x / 12 + y / 12) % 2 == 0 {
                image::Rgb([3, 8, 12])
            } else {
                image::Rgb([248, 245, 240])
            }
        });
        let different_path = tmp.path().join("different.png");
        different.save(&different_path).unwrap();
        let different_fp = fingerprint_image_sync(&different_path).unwrap();
        let different_evidence = perceptual_evidence_between(&different_fp, &base_fp);
        assert!(
            classify_perceptual(&different_evidence).is_none(),
            "different image must not enter candidate set: {different_evidence:?}"
        );
    }

    #[test]
    fn test_validate_source_directory() {
        let tmp = TempDir::new().unwrap();
        assert!(validate_source_directory(tmp.path().to_str().unwrap()).is_ok());
        assert!(validate_source_directory("/nonexistent/path").is_err());
    }

    #[test]
    fn test_scan_source_info() {
        let tmp = TempDir::new().unwrap();
        create_test_album(tmp.path(), "album_1");
        create_test_album(tmp.path(), "album_2");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let info = rt
            .block_on(scan_source_info(tmp.path().to_str().unwrap()))
            .unwrap();
        assert_eq!(info.album_count, 2);
        assert!(info.albums.contains(&"album_1".to_string()));
        assert!(info.albums.contains(&"album_2".to_string()));
    }

    #[test]
    fn test_supported_extensions() {
        assert!(SUPPORTED_IMAGE_EXTENSIONS.contains(&"jpg"));
        assert!(SUPPORTED_IMAGE_EXTENSIONS.contains(&"jpeg"));
        assert!(SUPPORTED_IMAGE_EXTENSIONS.contains(&"png"));
        assert!(SUPPORTED_IMAGE_EXTENSIONS.contains(&"webp"));
        assert!(!SUPPORTED_IMAGE_EXTENSIONS.contains(&"bmp"));
        assert!(!SUPPORTED_IMAGE_EXTENSIONS.contains(&"gif"));
    }

    #[test]
    fn test_classify_perceptual_near_auto() {
        let evidence = PerceptualEvidence {
            perceptual_eligible: true,
            block_distance: 1,
            double_gradient_distance: 2,
            block_distance_ratio: 0.03,
            double_gradient_distance_ratio: 0.04,
            transform_type: TransformType::Identity,
            confidence: 0.95,
        };
        let (mt, dec, src) = classify_perceptual(&evidence).unwrap();
        assert_eq!(mt, MatchType::PerceptualNear);
        assert_eq!(dec, Some(Decision::AutoDuplicate));
        assert_eq!(src, Some(DecisionSource::PerceptualRule));
    }

    #[test]
    fn test_classify_perceptual_review() {
        let evidence = PerceptualEvidence {
            perceptual_eligible: true,
            block_distance: 5,
            double_gradient_distance: 5,
            block_distance_ratio: 0.12,
            double_gradient_distance_ratio: 0.08,
            transform_type: TransformType::Identity,
            confidence: 0.8,
        };
        let (mt, dec, src) = classify_perceptual(&evidence).unwrap();
        assert_eq!(mt, MatchType::PerceptualSimilar);
        assert_eq!(dec, None);
        assert_eq!(src, None);
    }

    #[test]
    fn test_classify_perceptual_rejects_outside_review_threshold() {
        let evidence = PerceptualEvidence {
            perceptual_eligible: true,
            block_distance: 6,
            double_gradient_distance: 7,
            block_distance_ratio: 0.121,
            double_gradient_distance_ratio: 0.05,
            transform_type: TransformType::Rot90,
            confidence: 0.7,
        };
        assert!(classify_perceptual(&evidence).is_none());
    }

    #[test]
    fn perceptually_ineligible_images_never_auto_duplicate() {
        let evidence = PerceptualEvidence {
            perceptual_eligible: false,
            block_distance: 0,
            double_gradient_distance: 0,
            block_distance_ratio: 0.0,
            double_gradient_distance_ratio: 0.0,
            transform_type: TransformType::Identity,
            confidence: 1.0,
        };

        assert!(classify_perceptual(&evidence).is_none());
    }

    /// Real PostgreSQL + filesystem scan integration test.
    ///
    /// Invocation:
    ///   IMAGEDB_POSTGRES_BIN=/path/to/pgsql/bin cargo test \
    ///       --manifest-path apps/desktop/src-tauri/Cargo.toml \
    ///       real_scan_persists_exact_duplicates -- --ignored --test-threads=1
    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_scan_persists_exact_duplicates() {
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            panic!(
                "IMAGEDB_POSTGRES_BIN is not set; cannot run the real scan integration test. \
                 Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run \
                 `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime \
                 at .local/db-tools/postgresql-18.4/pgsql/bin."
            );
        }

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let album = create_test_album(&source_root, "album_a");

        let original = create_test_image(&album, "original.png");
        let renamed = album.join("renamed.png");
        std::fs::copy(&original, &renamed).unwrap();
        let metadata_variant = album.join("metadata.png");
        write_png_with_text_chunk(&original, &metadata_variant);

        let original_bytes_before = std::fs::read(&original).unwrap();
        let renamed_bytes_before = std::fs::read(&renamed).unwrap();
        let metadata_bytes_before = std::fs::read(&metadata_variant).unwrap();

        let fp_original = fingerprint_image_sync(&original).unwrap();
        let fp_metadata = fingerprint_image_sync(&metadata_variant).unwrap();
        assert_ne!(fp_original.blake3_bytes, fp_metadata.blake3_bytes);
        assert_eq!(fp_original.pixel_hash_bytes, fp_metadata.pixel_hash_bytes);

        std::fs::create_dir_all(&app_data).unwrap();
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

        let empty_db_info = ImportRepository::get_database_info_dashboard(
            &client,
            crate::repositories::import_repository::DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: Some("0015_fingerprint_v2".to_string()),
            },
        )
        .await
        .unwrap();
        assert_eq!(empty_db_info.library.library_root_count, 0);
        assert_eq!(empty_db_info.imports.import_run_count, 0);
        assert!(empty_db_info.latest_run.is_none());

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
            &album.display().to_string(),
            "album_a",
        )
        .await
        .unwrap();

        let scanned_images = scan_album_for_images(&album, "album_a").unwrap();
        assert_eq!(scanned_images.len(), 3);

        struct PersistedImage {
            id: Uuid,
            fp: FingerprintedData,
        }

        let mut persisted = Vec::new();
        for image in scanned_images {
            let fp = fingerprint_image_sync(&image.source_path).unwrap();
            let id = ImportRepository::insert_import_image(
                &client,
                NewImportImage {
                    album_id,
                    source_path: image.source_path.display().to_string(),
                    relative_path: image.relative_path,
                    file_size: image.file_size,
                    modified_at: image.modified_at,
                    width: Some(fp.width as i32),
                    height: Some(fp.height as i32),
                    format: Some(fp.format.clone()),
                    decode_state: DecodeState::Decoded,
                    blake3: Some(fp.blake3_bytes.clone()),
                    pixel_hash: Some(fp.pixel_hash_bytes.clone()),
                    block_hash_16: Some(fp.block_hash_16.clone()),
                    double_gradient_hash_32: Some(fp.double_gradient_hash_32.clone()),
                    perceptual_eligible: fp.perceptual_eligible,
                    fingerprint_version: Some("2".to_string()),
                    state: ImportImageState::Fingerprinted,
                },
            )
            .await
            .unwrap();
            persisted.push(PersistedImage { id, fp });
        }

        let mut duplicate_count = 0u32;
        for i in 0..persisted.len() {
            for j in (i + 1)..persisted.len() {
                let a = &persisted[i];
                let b = &persisted[j];
                let file_exact =
                    a.fp.file_size == b.fp.file_size && a.fp.blake3_bytes == b.fp.blake3_bytes;
                let pixel_exact = a.fp.pixel_hash_bytes == b.fp.pixel_hash_bytes;

                if file_exact {
                    ImportRepository::insert_duplicate_candidate(
                        &client,
                        NewDuplicateCandidate {
                            import_run_id,
                            source_image_id: a.id,
                            candidate_source_image_id: Some(b.id),
                            candidate_library_image_id: None,
                            scope: DuplicateScope::IntraAlbum,
                            match_type: MatchType::FileExact,
                            blake3_equal: true,
                            pixel_hash_equal: pixel_exact,
                            block_distance: None,
                            double_gradient_distance: None,
                            block_distance_ratio: None,
                            double_gradient_distance_ratio: None,
                            transform_type: None,
                            confidence: Some(1.0),
                            decision: Some(Decision::AutoDuplicate),
                            decision_source: Some(DecisionSource::ExactRule),
                        },
                    )
                    .await
                    .unwrap();
                    duplicate_count += 1;
                } else if pixel_exact {
                    ImportRepository::insert_duplicate_candidate(
                        &client,
                        NewDuplicateCandidate {
                            import_run_id,
                            source_image_id: a.id,
                            candidate_source_image_id: Some(b.id),
                            candidate_library_image_id: None,
                            scope: DuplicateScope::IntraAlbum,
                            match_type: MatchType::PixelExact,
                            blake3_equal: false,
                            pixel_hash_equal: true,
                            block_distance: None,
                            double_gradient_distance: None,
                            block_distance_ratio: None,
                            double_gradient_distance_ratio: None,
                            transform_type: None,
                            confidence: Some(1.0),
                            decision: Some(Decision::AutoDuplicate),
                            decision_source: Some(DecisionSource::ExactRule),
                        },
                    )
                    .await
                    .unwrap();
                    duplicate_count += 1;
                }
            }
        }

        let statistics = serde_json::json!({
            "total_albums": 1,
            "total_images": persisted.len(),
            "duplicate_count": duplicate_count,
            "error_count": 0,
        });
        ImportRepository::update_import_run_statistics(&client, import_run_id, &statistics)
            .await
            .unwrap();
        ImportRepository::update_import_run_state(
            &client,
            import_run_id,
            &ImportRunState::Completed,
        )
        .await
        .unwrap();

        let image_count: i64 = client
            .query_one(
                "SELECT COUNT(*)
                 FROM import_images ii
                 JOIN import_albums ia ON ia.id = ii.import_album_id
                 WHERE ia.import_run_id = $1",
                &[&import_run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(image_count, 3);

        let file_exact_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM duplicate_candidates
                 WHERE import_run_id = $1 AND scope = 'intra_album'
                 AND match_type = 'file_exact' AND blake3_equal = TRUE",
                &[&import_run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert!(file_exact_count >= 1);

        let pixel_exact_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM duplicate_candidates
                 WHERE import_run_id = $1 AND scope = 'intra_album'
                 AND match_type = 'pixel_exact' AND pixel_hash_equal = TRUE",
                &[&import_run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert!(pixel_exact_count >= 2);

        let library_image_count: i64 = client
            .query_one("SELECT COUNT(*) FROM library_images", &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(library_image_count, 0);

        let library_album_id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO library_albums
                    (id, library_root_id, display_name, relative_path, manifest_version,
                     manifest_hash, image_count, state)
                 VALUES ($1, $2, 'history', 'history', '1', $3, 51, 'committed')",
                &[&library_album_id, &library_root_id, &vec![7u8; 32]],
            )
            .await
            .unwrap();
        let exact_import = persisted.remove(0);
        let exact_library_id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO library_images
                    (id, album_id, relative_path, file_size, width, height, format,
                     blake3, pixel_hash, block_hash_16, double_gradient_hash_32,
                     fingerprint_version, state)
                 VALUES ($1, $2, 'exact.png', $3, $4, $5, $6, $7, $8, $9, $10,
                         '2', 'committed')",
                &[
                    &exact_library_id,
                    &library_album_id,
                    &(exact_import.fp.file_size as i64),
                    &(exact_import.fp.width as i32),
                    &(exact_import.fp.height as i32),
                    &exact_import.fp.format,
                    &exact_import.fp.blake3_bytes,
                    &exact_import.fp.pixel_hash_bytes,
                    &exact_import.fp.block_hash_16,
                    &exact_import.fp.double_gradient_hash_32,
                ],
            )
            .await
            .unwrap();
        let mut decoy_ids = Vec::new();
        for index in 0..50u8 {
            let decoy_id = Uuid::new_v4();
            client
                .execute(
                    "INSERT INTO library_images
                        (id, album_id, relative_path, file_size, width, height, format,
                         blake3, pixel_hash, block_hash_16, double_gradient_hash_32,
                         fingerprint_version, state)
                     VALUES ($1, $2, $3, 1, 1, 1, 'png', $4, $5, $6, $7,
                             '2', 'committed')",
                    &[
                        &decoy_id,
                        &library_album_id,
                        &format!("decoy-{index}.png"),
                        &vec![index; 32],
                        &vec![index; 32],
                        &vec![index.wrapping_add(1); 32],
                        &vec![index.wrapping_add(2); 68],
                    ],
                )
                .await
                .unwrap();
            decoy_ids.push(decoy_id);
        }

        let evidence_source_id = persisted[0].id;
        let evidence_library_id = decoy_ids[0];
        let inserted = ImportRepository::upsert_duplicate_candidates_batch(
            &client,
            &[NewDuplicateCandidate {
                import_run_id,
                source_image_id: evidence_source_id,
                candidate_source_image_id: None,
                candidate_library_image_id: Some(evidence_library_id),
                scope: DuplicateScope::Library,
                match_type: MatchType::PerceptualSimilar,
                blake3_equal: false,
                pixel_hash_equal: false,
                block_distance: Some(12),
                double_gradient_distance: Some(20),
                block_distance_ratio: Some(12.0 / 256.0),
                double_gradient_distance_ratio: Some(20.0 / 544.0),
                transform_type: Some(TransformType::Rot90.to_string()),
                confidence: Some(weighted_similarity(12.0 / 256.0, 20.0 / 544.0)),
                decision: None,
                decision_source: None,
            }],
        )
        .await
        .unwrap();
        assert_eq!(inserted, 1);
        let evidence = client
            .query_one(
                "SELECT block_distance, double_gradient_distance,
                        block_distance_ratio, double_gradient_distance_ratio,
                        transform_type, confidence
                 FROM duplicate_candidates
                 WHERE import_run_id = $1 AND source_image_id = $2
                   AND candidate_library_image_id = $3",
                &[&import_run_id, &evidence_source_id, &evidence_library_id],
            )
            .await
            .unwrap();
        assert_eq!(evidence.get::<_, i32>("block_distance"), 12);
        assert_eq!(evidence.get::<_, i32>("double_gradient_distance"), 20);
        assert_eq!(evidence.get::<_, String>("transform_type"), "rot90");
        assert!((evidence.get::<_, f64>("block_distance_ratio") - 12.0 / 256.0).abs() < 1e-12);
        assert!(
            (evidence.get::<_, f64>("double_gradient_distance_ratio") - 20.0 / 544.0).abs() < 1e-12
        );
        assert!(
            (evidence.get::<_, f64>("confidence")
                - weighted_similarity(12.0 / 256.0, 20.0 / 544.0))
            .abs()
                < 1e-12
        );

        let exact_entry = AlbumImageEntry {
            image_db_id: exact_import.id,
            fp: exact_import.fp,
        };
        let cancelled = AtomicBool::new(false);
        let library_index = Arc::new(RwLock::new(None));
        let run_exact_index = RunExactRepresentativeIndex::default();
        let detection_ctx = AlbumDetectionContext {
            client: &client,
            import_run_id,
            album_id,
            cancelled: &cancelled,
            library_index: &library_index,
            run_exact_index: &run_exact_index,
        };
        let mut detected = 0;
        let mut metrics = AlbumAnalysisMetrics::default();
        let mut detection_progress = ScanProgressEvent {
            state: "running".to_string(),
            import_run_id: Some(import_run_id.to_string()),
            current_stage: "detecting_duplicates".to_string(),
            current_album: Some("album_a".to_string()),
            processed_images: 1,
            total_albums: 1,
            total_images: 1,
            duplicate_count: 0,
            error_count: 0,
            errors: vec![],
        };
        detect_album_duplicates(
            &detection_ctx,
            &[&exact_entry],
            &mut detected,
            &mut detection_progress,
            &mut metrics,
        )
        .await
        .unwrap();
        let exact_pair_rows: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM duplicate_candidates
                 WHERE import_run_id = $1 AND source_image_id = $2
                   AND candidate_library_image_id = $3",
                &[&import_run_id, &exact_entry.image_db_id, &exact_library_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(exact_pair_rows, 1);
        let exact_pair_type: String = client
            .query_one(
                "SELECT match_type FROM duplicate_candidates
                 WHERE import_run_id = $1 AND source_image_id = $2
                   AND candidate_library_image_id = $3",
                &[&import_run_id, &exact_entry.image_db_id, &exact_library_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(exact_pair_type, "file_exact");

        drop(client);
        db_handle.abort();
        manager.shutdown().await.unwrap();

        assert_eq!(std::fs::read(&original).unwrap(), original_bytes_before);
        assert_eq!(std::fs::read(&renamed).unwrap(), renamed_bytes_before);
        assert_eq!(
            std::fs::read(&metadata_variant).unwrap(),
            metadata_bytes_before
        );
    }

    /// Real PostgreSQL album-workflow checkpoint test.
    ///
    /// Covers stale resume cleanup, failed-album retry isolation, dashboard
    /// counters, and review entry from already-persisted candidates.
    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_scan_album_workflow_resume_cleanup_and_retry() {
        use crate::domain::import_state::ReviewDecisionAction;
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            panic!(
                "IMAGEDB_POSTGRES_BIN is not set; cannot run the real album workflow test. \
                 Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run \
                 `node scripts/package-postgres-runtime.mjs`."
            );
        }

        async fn insert_test_image(
            client: &tokio_postgres::Client,
            album_id: Uuid,
            path: &Path,
            relative_path: &str,
        ) -> Uuid {
            let fp = fingerprint_image_sync(path).unwrap();
            ImportRepository::insert_import_image(
                client,
                NewImportImage {
                    album_id,
                    source_path: path.display().to_string(),
                    relative_path: relative_path.to_string(),
                    file_size: fp.file_size as i64,
                    modified_at: None,
                    width: Some(fp.width as i32),
                    height: Some(fp.height as i32),
                    format: Some(fp.format.clone()),
                    decode_state: DecodeState::Decoded,
                    blake3: Some(fp.blake3_bytes),
                    pixel_hash: Some(fp.pixel_hash_bytes),
                    block_hash_16: Some(fp.block_hash_16),
                    double_gradient_hash_32: Some(fp.double_gradient_hash_32),
                    perceptual_eligible: fp.perceptual_eligible,
                    fingerprint_version: Some("2".to_string()),
                    state: ImportImageState::Fingerprinted,
                },
            )
            .await
            .unwrap()
        }

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let done_album_dir = create_test_album(&source_root, "done_album");
        let stale_album_dir = create_test_album(&source_root, "stale_album");
        let failed_album_dir = create_test_album(&source_root, "failed_album");
        let done_a = create_test_image(&done_album_dir, "a.png");
        let done_b = create_test_image(&done_album_dir, "b.png");
        let stale_img = create_test_image(&stale_album_dir, "partial.png");
        let failed_img = create_test_image(&failed_album_dir, "failed.png");

        std::fs::create_dir_all(&app_data).unwrap();
        let mut manager = PostgresManager::new(&app_data);
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok, "diagnostics: {:?}", probe.diagnostics);

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
        let done_album_id = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            &done_album_dir.display().to_string(),
            "done_album",
        )
        .await
        .unwrap();
        let stale_album_id = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            &stale_album_dir.display().to_string(),
            "stale_album",
        )
        .await
        .unwrap();
        let failed_album_id = ImportRepository::insert_import_album(
            &client,
            import_run_id,
            &failed_album_dir.display().to_string(),
            "failed_album",
        )
        .await
        .unwrap();

        let done_image_a =
            insert_test_image(&client, done_album_id, &done_a, "done_album/a.png").await;
        let done_image_b =
            insert_test_image(&client, done_album_id, &done_b, "done_album/b.png").await;
        let stale_image = insert_test_image(
            &client,
            stale_album_id,
            &stale_img,
            "stale_album/partial.png",
        )
        .await;
        let failed_image = insert_test_image(
            &client,
            failed_album_id,
            &failed_img,
            "failed_album/failed.png",
        )
        .await;

        let review_candidate_id = ImportRepository::insert_duplicate_candidate(
            &client,
            NewDuplicateCandidate {
                import_run_id,
                source_image_id: done_image_a,
                candidate_source_image_id: Some(done_image_b),
                candidate_library_image_id: None,
                scope: DuplicateScope::IntraAlbum,
                match_type: MatchType::PerceptualSimilar,
                blake3_equal: false,
                pixel_hash_equal: false,
                block_distance: Some(8),
                double_gradient_distance: Some(8),
                block_distance_ratio: Some(8.0 / 256.0),
                double_gradient_distance_ratio: Some(8.0 / 544.0),
                transform_type: None,
                confidence: Some(0.8),
                decision: None,
                decision_source: None,
            },
        )
        .await
        .unwrap();
        ImportRepository::insert_duplicate_candidate(
            &client,
            NewDuplicateCandidate {
                import_run_id,
                source_image_id: stale_image,
                candidate_source_image_id: Some(done_image_a),
                candidate_library_image_id: None,
                scope: DuplicateScope::CrossAlbum,
                match_type: MatchType::FileExact,
                blake3_equal: true,
                pixel_hash_equal: false,
                block_distance: None,
                double_gradient_distance: None,
                block_distance_ratio: None,
                double_gradient_distance_ratio: None,
                transform_type: None,
                confidence: Some(1.0),
                decision: Some(Decision::AutoDuplicate),
                decision_source: Some(DecisionSource::ExactRule),
            },
        )
        .await
        .unwrap();
        ImportRepository::insert_duplicate_candidate(
            &client,
            NewDuplicateCandidate {
                import_run_id,
                source_image_id: failed_image,
                candidate_source_image_id: Some(done_image_b),
                candidate_library_image_id: None,
                scope: DuplicateScope::CrossAlbum,
                match_type: MatchType::FileExact,
                blake3_equal: true,
                pixel_hash_equal: false,
                block_distance: None,
                double_gradient_distance: None,
                block_distance_ratio: None,
                double_gradient_distance_ratio: None,
                transform_type: None,
                confidence: Some(1.0),
                decision: Some(Decision::AutoDuplicate),
                decision_source: Some(DecisionSource::ExactRule),
            },
        )
        .await
        .unwrap();

        ImportRepository::mark_import_album_analyzing(&client, done_album_id)
            .await
            .unwrap();
        let done_status = ImportRepository::finalize_import_album_analysis(&client, done_album_id)
            .await
            .unwrap();
        assert_eq!(
            done_status.state,
            ImportAlbumState::ReviewRequired.to_string()
        );
        assert_eq!(done_status.image_count, 2);
        assert_eq!(done_status.review_candidate_count, 1);

        ImportRepository::mark_import_album_analyzing(&client, stale_album_id)
            .await
            .unwrap();
        let (stale_snapshot_id, stale_snapshot_hash) =
            crate::services::source_snapshot_service::capture_source_album_snapshot(
                &client,
                import_run_id,
                stale_album_id,
                &stale_album_dir,
            )
            .await
            .unwrap();
        ImportRepository::mark_import_album_failed(
            &client,
            failed_album_id,
            "TEST_FAILURE",
            "simulated album failure",
        )
        .await
        .unwrap();

        let analyzing_after_review_refresh =
            ImportRepository::refresh_review_album_and_run(&client, stale_album_id)
                .await
                .unwrap();
        assert_eq!(
            analyzing_after_review_refresh.state,
            ImportAlbumState::Analyzing.to_string(),
            "async review must not finalize an in-flight album"
        );
        let failed_after_review_refresh =
            ImportRepository::refresh_review_album_and_run(&client, failed_album_id)
                .await
                .unwrap();
        assert_eq!(
            failed_after_review_refresh.state,
            ImportAlbumState::Failed.to_string(),
            "review counter refresh must preserve a failed checkpoint"
        );

        let cleaned = ImportRepository::mark_stale_analyzing_albums(&client, import_run_id)
            .await
            .unwrap();
        assert_eq!(cleaned, 1);

        let stale_status = ImportRepository::get_import_album_status_by_id(&client, stale_album_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stale_status.state, ImportAlbumState::Pending.to_string());
        assert_eq!(stale_status.image_count, 0);
        assert_eq!(stale_status.duplicate_candidate_count, 0);
        let stale_image_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM import_images WHERE import_album_id = $1",
                &[&stale_album_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(stale_image_count, 0);
        let preserved_snapshot =
            ImportRepository::get_source_album_snapshot(&client, stale_album_id)
                .await
                .unwrap()
                .expect("stale cleanup must preserve the immutable source snapshot");
        assert_eq!(preserved_snapshot.snapshot_id, stale_snapshot_id);
        let (reused_snapshot_id, reused_snapshot_hash) =
            crate::services::source_snapshot_service::capture_source_album_snapshot(
                &client,
                import_run_id,
                stale_album_id,
                &stale_album_dir,
            )
            .await
            .unwrap();
        assert_eq!(reused_snapshot_id, stale_snapshot_id);
        assert_eq!(reused_snapshot_hash, stale_snapshot_hash);
        assert!(
            crate::services::source_snapshot_service::verify_source_album_snapshot(
                &client,
                stale_album_id,
                &stale_album_dir,
            )
            .await
            .unwrap()
            .is_empty()
        );

        let done_image_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM import_images WHERE import_album_id = $1",
                &[&done_album_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(done_image_count, 2);
        let remaining_review_candidates: i64 = client
            .query_one(
                "SELECT COUNT(*)
                 FROM duplicate_candidates
                 WHERE import_run_id = $1 AND decision IS NULL",
                &[&import_run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(remaining_review_candidates, 1);

        let non_failed_retry =
            ImportRepository::reset_failed_album_for_retry(&client, done_album_id).await;
        assert!(non_failed_retry.is_err());
        let done_image_count_after_rejected_retry: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM import_images WHERE import_album_id = $1",
                &[&done_album_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(done_image_count_after_rejected_retry, 2);

        ImportRepository::reset_failed_album_for_retry(&client, failed_album_id)
            .await
            .unwrap();
        let failed_status =
            ImportRepository::get_import_album_status_by_id(&client, failed_album_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(failed_status.state, ImportAlbumState::Pending.to_string());
        assert!(failed_status.last_error_message.is_none());
        let failed_image_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM import_images WHERE import_album_id = $1",
                &[&failed_album_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(failed_image_count, 0);

        let summary = ImportRepository::list_import_runs_summary(&client)
            .await
            .unwrap();
        let run_summary = summary
            .iter()
            .find(|item| item.import_run_id == import_run_id.to_string())
            .unwrap();
        assert_eq!(run_summary.total_albums, 3);
        assert_eq!(run_summary.pending_albums, 2);
        assert_eq!(run_summary.review_required_albums, 1);
        assert_eq!(run_summary.pending_reviews, 1);

        let latest_reviewable = ImportRepository::get_latest_reviewable_run(&client)
            .await
            .unwrap();
        assert_eq!(latest_reviewable, Some(import_run_id));

        let stale_statistics = serde_json::json!({
            "total_albums": 999,
            "total_images": 999,
            "duplicate_count": 999,
            "error_count": 999,
        });
        ImportRepository::update_import_run_statistics(&client, import_run_id, &stale_statistics)
            .await
            .unwrap();
        let refreshed_statistics =
            ImportRepository::refresh_import_run_statistics(&client, import_run_id)
                .await
                .unwrap();
        assert_eq!(refreshed_statistics["total_albums"], 3);
        assert_eq!(refreshed_statistics["total_images"], 2);
        assert_eq!(refreshed_statistics["duplicate_count"], 1);
        assert_eq!(refreshed_statistics["pending_review_count"], 1);

        let db_info = ImportRepository::get_database_info_dashboard(
            &client,
            crate::repositories::import_repository::DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: Some("0012_album_workflow_repair".to_string()),
            },
        )
        .await
        .unwrap();
        assert_eq!(db_info.library.library_root_count, 1);
        assert_eq!(db_info.imports.import_run_count, 1);
        assert_eq!(db_info.imports.import_album_count, 3);
        assert_eq!(db_info.imports.import_image_count, 2);
        assert_eq!(db_info.imports.pending_review_count, 1);
        assert_eq!(db_info.imports.failed_album_count, 0);
        assert_eq!(
            db_info.latest_run.unwrap().import_run_id,
            import_run_id.to_string()
        );

        crate::services::review_service::submit_decision(
            &client,
            review_candidate_id,
            ReviewDecisionAction::KeepSource,
        )
        .await
        .unwrap();
        let done_status = ImportRepository::get_import_album_status_by_id(&client, done_album_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(done_status.review_candidate_count, 0);
        assert_eq!(done_status.state, ImportAlbumState::Analyzed.to_string());
        let summary = ImportRepository::list_import_runs_summary(&client)
            .await
            .unwrap();
        let run_summary = summary
            .iter()
            .find(|item| item.import_run_id == import_run_id.to_string())
            .unwrap();
        assert_eq!(run_summary.pending_reviews, 0);

        ImportRepository::update_import_run_state(&client, import_run_id, &ImportRunState::Failed)
            .await
            .unwrap();
        ImportRepository::abandon_import_run(&client, import_run_id)
            .await
            .unwrap();
        drop(client);
        db_handle.abort();

        let manager = Arc::new(Mutex::new(manager));
        let settings = Arc::new(Mutex::new(
            crate::infrastructure::settings::SettingsStore::new(&app_data).unwrap(),
        ));
        let tracker = Arc::new(Mutex::new(ScanProgress::idle()));
        let clean_scan = run_scan(
            manager.clone(),
            settings,
            Arc::new(RwLock::new(None)),
            source_root.display().to_string(),
            Arc::new(AtomicBool::new(false)),
            tracker,
        )
        .await
        .unwrap();
        let new_run_id = Uuid::parse_str(clean_scan.import_run_id.as_deref().unwrap()).unwrap();
        assert_ne!(new_run_id, import_run_id);
        let (verify_client, verify_handle) = {
            let manager = manager.lock().await;
            manager.connect().await.unwrap()
        };
        let old_state: String = verify_client
            .query_one(
                "SELECT state FROM import_runs WHERE id = $1",
                &[&import_run_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(old_state, "abandoned");
        let same_root_runs: i64 = verify_client
            .query_one(
                "SELECT COUNT(*) FROM import_runs WHERE source_root = $1",
                &[&source_root.display().to_string()],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(same_root_runs, 2, "ordinary start must create a new run");
        drop(verify_client);
        verify_handle.abort();
        manager.lock().await.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_abandoned_runs_are_history_not_actionable_workflow() {
        use crate::infrastructure::postgres::PostgresManager;
        use crate::repositories::import_repository::DatabaseInfoDatabaseSection;

        let tmp = TempDir::new().unwrap();
        let mut manager = PostgresManager::new(tmp.path());
        manager.initialize().await.unwrap();
        let (client, handle) = manager.connect().await.unwrap();
        let root_id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        let old_run = Uuid::new_v4();
        let new_run = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO import_runs
                (id, source_root, library_root_id, state, policy_version, started_at)
             VALUES ($1, 'C:/old', $3, 'abandoned', 'test', now() - interval '1 hour'),
                    ($2, 'C:/new', $3, 'ready_to_commit', 'test', now())",
                &[&old_run, &new_run, &root_id],
            )
            .await
            .unwrap();
        let old_album = Uuid::new_v4();
        let new_album = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO import_albums
                (id, import_run_id, source_path, source_name, state)
             VALUES ($1, $2, 'C:/old/failed', 'old-failed', 'failed'),
                    ($3, $4, 'C:/new/ready', 'new-ready', 'analyzed')",
                &[&old_album, &old_run, &new_album, &new_run],
            )
            .await
            .unwrap();
        let old_a = Uuid::new_v4();
        let old_b = Uuid::new_v4();
        let new_a = Uuid::new_v4();
        let new_b = Uuid::new_v4();
        for (id, album, path) in [
            (old_a, old_album, "old-a.png"),
            (old_b, old_album, "old-b.png"),
            (new_a, new_album, "new-a.png"),
            (new_b, new_album, "new-b.png"),
        ] {
            client.execute(
                "INSERT INTO import_images
                    (id, import_album_id, source_path, relative_path, file_size, decode_state, state)
                 VALUES ($1, $2, $3, $3, 1, 'decoded', 'fingerprinted')",
                &[&id, &album, &path],
            ).await.unwrap();
        }
        client
            .execute(
                "INSERT INTO duplicate_candidates
                (id, import_run_id, source_image_id, candidate_source_image_id, scope, match_type)
             VALUES ($1, $2, $3, $4, 'cross_album', 'perceptual_similar')",
                &[&Uuid::new_v4(), &old_run, &old_a, &old_b],
            )
            .await
            .unwrap();

        let dashboard = ImportRepository::get_database_info_dashboard(
            &client,
            DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: Some(
                    "0014_candidate_review_semantics_and_abandoned_filters".to_string(),
                ),
            },
        )
        .await
        .unwrap();
        assert_eq!(dashboard.imports.import_run_count, 2);
        assert_eq!(dashboard.imports.import_album_count, 2);
        assert_eq!(dashboard.imports.import_image_count, 4);
        assert_eq!(dashboard.imports.pending_review_count, 0);
        assert_eq!(dashboard.imports.failed_album_count, 0);
        assert_eq!(
            dashboard.latest_run.as_ref().unwrap().import_run_id,
            new_run.to_string()
        );
        assert_eq!(
            dashboard
                .latest_actionable_run
                .as_ref()
                .unwrap()
                .run
                .import_run_id,
            new_run.to_string()
        );
        assert_eq!(
            dashboard.next_action,
            crate::repositories::import_repository::DashboardNextAction::GeneratePlan
        );
        assert_eq!(
            ImportRepository::get_latest_reviewable_run(&client)
                .await
                .unwrap(),
            Some(new_run)
        );
        let old_evidence: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM duplicate_candidates WHERE import_run_id = $1",
                &[&old_run],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(old_evidence, 1);

        client
            .execute(
                "UPDATE import_runs SET state = 'review_required' WHERE id = $1",
                &[&new_run],
            )
            .await
            .unwrap();
        let dashboard = ImportRepository::get_database_info_dashboard(
            &client,
            DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            dashboard.next_action,
            crate::repositories::import_repository::DashboardNextAction::GeneratePlan,
            "review_required with no pending reviews must return to plan generation"
        );

        let new_candidate = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO duplicate_candidates
                (id, import_run_id, source_image_id, candidate_source_image_id, scope, match_type)
             VALUES ($1, $2, $3, $4, 'cross_album', 'perceptual_similar')",
                &[&new_candidate, &new_run, &new_a, &new_b],
            )
            .await
            .unwrap();
        ImportRepository::refresh_album_workflow_summary(&client, new_album)
            .await
            .unwrap();
        let dashboard = ImportRepository::get_database_info_dashboard(
            &client,
            DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            dashboard.next_action,
            crate::repositories::import_repository::DashboardNextAction::Review
        );
        assert_eq!(
            ImportRepository::get_latest_reviewable_run(&client)
                .await
                .unwrap(),
            Some(new_run)
        );

        client
            .execute(
                "DELETE FROM duplicate_candidates WHERE id = $1",
                &[&new_candidate],
            )
            .await
            .unwrap();
        ImportRepository::refresh_album_workflow_summary(&client, new_album)
            .await
            .unwrap();
        let plan_id = ImportRepository::create_import_plan(&client, new_run, 1, "test", root_id)
            .await
            .unwrap();
        let second_album = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO import_albums
                 (id, import_run_id, source_path, source_name, state)
                 VALUES ($1, $2, 'C:/new/second', 'new-second', 'analyzed')",
                &[&second_album, &new_run],
            )
            .await
            .unwrap();
        ImportRepository::insert_plan_album(&client, plan_id, new_album, "new-ready", 0)
            .await
            .unwrap();
        ImportRepository::insert_plan_album(&client, plan_id, second_album, "new-second", 0)
            .await
            .unwrap();
        ImportRepository::set_plan_hash(&client, plan_id, &[7_u8; 32])
            .await
            .unwrap();
        ImportRepository::update_import_plan_state(
            &client,
            plan_id,
            &crate::domain::state_machine::PlanState::Frozen,
        )
        .await
        .unwrap();
        client
            .execute(
                "UPDATE import_runs SET state = 'cancelled' WHERE id = $1",
                &[&new_run],
            )
            .await
            .unwrap();
        let dashboard = ImportRepository::get_database_info_dashboard(
            &client,
            DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: None,
            },
        )
        .await
        .unwrap();
        let actionable = dashboard.latest_actionable_run.as_ref().unwrap();
        assert_eq!(
            dashboard.next_action,
            crate::repositories::import_repository::DashboardNextAction::ResumeCommit
        );
        assert!(actionable.has_frozen_plan);
        assert!(!actionable.has_recoverable_transaction);
        assert!(!actionable.has_terminal_unresolved_transaction);
        assert!(actionable.has_missing_plan_album_transaction);

        client
            .execute(
                "UPDATE import_runs SET state = 'committing' WHERE id = $1",
                &[&new_run],
            )
            .await
            .unwrap();
        let dashboard = ImportRepository::get_database_info_dashboard(
            &client,
            DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            dashboard.next_action,
            crate::repositories::import_repository::DashboardNextAction::ResumeCommit,
            "committing before the first transaction prewrite must resume commit"
        );
        assert_eq!(
            ImportRepository::get_latest_committable_run(&client)
                .await
                .unwrap(),
            Some(new_run)
        );

        let first_transaction = Uuid::new_v4();
        ImportRepository::insert_file_transaction(
            &client,
            first_transaction,
            new_run,
            new_album,
            &crate::domain::state_machine::TransactionState::Planned,
            Some("C:/staging"),
            Some("C:/target"),
            None,
        )
        .await
        .unwrap();
        client
            .execute(
                "UPDATE import_runs SET state = 'committing' WHERE id = $1",
                &[&new_run],
            )
            .await
            .unwrap();
        let dashboard = ImportRepository::get_database_info_dashboard(
            &client,
            DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            dashboard.next_action,
            crate::repositories::import_repository::DashboardNextAction::Recover
        );
        assert!(
            dashboard
                .latest_actionable_run
                .as_ref()
                .unwrap()
                .has_recoverable_transaction
        );

        ImportRepository::update_file_transaction_state(
            &client,
            first_transaction,
            &crate::domain::state_machine::TransactionState::SourceArchived,
            None,
        )
        .await
        .unwrap();
        client
            .execute(
                "UPDATE import_runs SET state = 'recovery_required' WHERE id = $1",
                &[&new_run],
            )
            .await
            .unwrap();
        let dashboard = ImportRepository::get_database_info_dashboard(
            &client,
            DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            dashboard.next_action,
            crate::repositories::import_repository::DashboardNextAction::ResumeCommit,
            "a completed first album plus a missing second transaction must resume commit"
        );
        assert!(
            dashboard
                .latest_actionable_run
                .as_ref()
                .unwrap()
                .has_missing_plan_album_transaction
        );
        assert_eq!(
            ImportRepository::get_latest_committable_run(&client)
                .await
                .unwrap(),
            Some(new_run)
        );

        let failed_transaction = Uuid::new_v4();
        ImportRepository::insert_file_transaction(
            &client,
            failed_transaction,
            new_run,
            second_album,
            &crate::domain::state_machine::TransactionState::SourceArchived,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let dashboard = ImportRepository::get_database_info_dashboard(
            &client,
            DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            dashboard.next_action,
            crate::repositories::import_repository::DashboardNextAction::ResumeCommit,
            "an all-archived stale parent run must re-enter commit for final reconciliation"
        );
        assert_eq!(
            ImportRepository::get_latest_committable_run(&client)
                .await
                .unwrap(),
            Some(new_run)
        );

        ImportRepository::update_file_transaction_state(
            &client,
            failed_transaction,
            &crate::domain::state_machine::TransactionState::Failed,
            Some("terminal failure"),
        )
        .await
        .unwrap();
        let dashboard = ImportRepository::get_database_info_dashboard(
            &client,
            DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: None,
            },
        )
        .await
        .unwrap();
        let actionable = dashboard.latest_actionable_run.as_ref().unwrap();
        assert_eq!(
            dashboard.next_action,
            crate::repositories::import_repository::DashboardNextAction::InspectTransactionFailure
        );
        assert!(!actionable.has_recoverable_transaction);
        assert!(actionable.has_terminal_unresolved_transaction);
        assert!(!actionable.has_missing_plan_album_transaction);
        assert_eq!(
            ImportRepository::get_latest_committable_run(&client)
                .await
                .unwrap(),
            None,
            "terminal-only unresolved transactions require manual disposition"
        );
        let diagnostics = crate::services::recovery_service::scan_recoverable_transactions(&client)
            .await
            .unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].transaction_id, failed_transaction);
        assert_eq!(diagnostics[0].current_state, "failed");

        ImportRepository::update_file_transaction_state(
            &client,
            failed_transaction,
            &crate::domain::state_machine::TransactionState::Cancelled,
            Some("terminal cancellation"),
        )
        .await
        .unwrap();
        let dashboard = ImportRepository::get_database_info_dashboard(
            &client,
            DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            dashboard.next_action,
            crate::repositories::import_repository::DashboardNextAction::InspectTransactionFailure
        );
        let diagnostics = crate::services::recovery_service::scan_recoverable_transactions(&client)
            .await
            .unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].current_state, "cancelled");

        drop(client);
        handle.abort();
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_scan_dashboard_source_files_removing_routes_to_recovery() {
        use crate::infrastructure::postgres::PostgresManager;
        use crate::repositories::import_repository::{
            DashboardNextAction, DatabaseInfoDatabaseSection,
        };

        let tmp = TempDir::new().unwrap();
        let mut manager = PostgresManager::new(tmp.path());
        manager.initialize().await.unwrap();
        let (client, handle) = manager.connect().await.unwrap();
        let root_id = ImportRepository::upsert_default_library_root(&client)
            .await
            .unwrap();
        let run_id = Uuid::new_v4();
        let album_id = Uuid::new_v4();
        let transaction_id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO import_runs
                 (id, source_root, library_root_id, state, policy_version)
                 VALUES ($1, 'C:/source', $2, 'recovery_required', 'test')",
                &[&run_id, &root_id],
            )
            .await
            .unwrap();
        client
            .execute(
                "INSERT INTO import_albums
                 (id, import_run_id, source_path, source_name, state)
                 VALUES ($1, $2, 'C:/source/album', 'album', 'analyzed')",
                &[&album_id, &run_id],
            )
            .await
            .unwrap();
        let plan_id = ImportRepository::create_import_plan(&client, run_id, 1, "test", root_id)
            .await
            .unwrap();
        ImportRepository::insert_plan_album(&client, plan_id, album_id, "album", 0)
            .await
            .unwrap();
        ImportRepository::set_plan_hash(&client, plan_id, &[9_u8; 32])
            .await
            .unwrap();
        ImportRepository::update_import_plan_state(
            &client,
            plan_id,
            &crate::domain::state_machine::PlanState::Frozen,
        )
        .await
        .unwrap();
        ImportRepository::insert_file_transaction(
            &client,
            transaction_id,
            run_id,
            album_id,
            &crate::domain::state_machine::TransactionState::SourceFilesRemoving,
            Some("C:/staging"),
            Some("C:/library/album"),
            None,
        )
        .await
        .unwrap();

        let dashboard = ImportRepository::get_database_info_dashboard(
            &client,
            DatabaseInfoDatabaseSection {
                mode: Some("managed_local".to_string()),
                status: "connected".to_string(),
                pgvector_available: true,
                migration_version: Some(
                    crate::infrastructure::postgres::MigrationRunner::latest_version().to_string(),
                ),
            },
        )
        .await
        .unwrap();
        assert_eq!(dashboard.next_action, DashboardNextAction::Recover);
        let actionable = dashboard.latest_actionable_run.as_ref().unwrap();
        assert_eq!(actionable.run.import_run_id, run_id.to_string());
        assert!(actionable.has_recoverable_transaction);
        assert_eq!(
            ImportRepository::get_latest_committable_run(&client)
                .await
                .unwrap(),
            None,
            "an active source cleanup must route to Recovery, never Commit"
        );

        let recoverable = ImportRepository::get_recoverable_transactions(&client)
            .await
            .unwrap();
        assert!(recoverable.iter().any(|row| row.id == transaction_id));

        drop(client);
        handle.abort();
        manager.shutdown().await.unwrap();
    }

    fn make_snapshot_file(path: &str, ft: &str, size: i64, hash: u8) -> NewSnapshotFile {
        NewSnapshotFile {
            relative_path: path.to_string(),
            file_type: ft.to_string(),
            file_size: size,
            blake3: vec![hash; 32],
        }
    }

    #[test]
    fn test_snapshot_hash_stable() {
        let files = vec![
            make_snapshot_file("a.jpg", "jpg", 100, 1),
            make_snapshot_file("b.png", "png", 200, 2),
        ];
        let h1 = compute_snapshot_hash(&files);
        let h2 = compute_snapshot_hash(&files);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 32);
    }

    #[test]
    fn test_snapshot_hash_changes_on_content_diff() {
        let files_a = vec![make_snapshot_file("a.jpg", "jpg", 100, 1)];
        let files_b = vec![make_snapshot_file("a.jpg", "jpg", 101, 1)];
        assert_ne!(
            compute_snapshot_hash(&files_a),
            compute_snapshot_hash(&files_b)
        );
    }

    #[test]
    fn test_snapshot_hash_changes_on_path_diff() {
        let files_a = vec![make_snapshot_file("a.jpg", "jpg", 100, 1)];
        let files_b = vec![make_snapshot_file("b.jpg", "jpg", 100, 1)];
        assert_ne!(
            compute_snapshot_hash(&files_a),
            compute_snapshot_hash(&files_b)
        );
    }

    #[test]
    fn test_snapshot_hash_order_independent_of_input_order() {
        let files_a = vec![
            make_snapshot_file("a.jpg", "jpg", 100, 1),
            make_snapshot_file("b.png", "png", 200, 2),
        ];
        let files_b = vec![
            make_snapshot_file("b.png", "png", 200, 2),
            make_snapshot_file("a.jpg", "jpg", 100, 1),
        ];
        assert_eq!(
            compute_snapshot_hash(&files_a),
            compute_snapshot_hash(&files_b)
        );
    }

    #[test]
    fn test_collect_album_files_covers_all_file_types() {
        let tmp = TempDir::new().unwrap();
        let album = tmp.path().join("my_album");
        std::fs::create_dir_all(&album).unwrap();

        create_test_image(&album, "img1.jpg");
        create_test_image(&album, "img2.png");
        std::fs::write(album.join("img3.webp"), b"fake webp content for test").unwrap();
        std::fs::write(album.join("description.txt"), b"some description").unwrap();
        let nested = album.join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("sidecar.xmp"), b"<xmp/>").unwrap();

        let files = collect_album_files(&album).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.relative_path.as_str()).collect();
        assert!(paths.contains(&"description.txt"));
        assert!(paths.contains(&"sub/sidecar.xmp"));
        assert_eq!(files.len(), 5);

        for f in &files {
            assert!(!f.relative_path.starts_with('/'));
            assert!(!f.relative_path.contains(".."));
            assert!(!f.relative_path.contains('\\'));
            assert!(!f.blake3.is_empty());
            assert!(f.file_size > 0);
        }
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_scan_persists_source_album_snapshot() {
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            panic!(
                "IMAGEDB_POSTGRES_BIN is not set; cannot run the real snapshot integration test. \
                 Set IMAGEDB_POSTGRES_BIN to a PostgreSQL 18.x bin directory, or run \
                 `node scripts/package-postgres-runtime.mjs` to populate the packaged runtime \
                 at .local/db-tools/postgresql-18.4/pgsql/bin."
            );
        }

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let album = create_test_album(&source_root, "snap_album");

        let img1 = create_test_image(&album, "img1.jpg");
        let img2 = create_test_image(&album, "img2.png");
        let img3 = album.join("img3.webp");
        std::fs::write(&img3, b"fake webp content for snapshot test").unwrap();
        std::fs::write(album.join("description.txt"), b"album notes").unwrap();
        let nested = album.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("meta.xmp"), b"<xmp>data</xmp>").unwrap();

        let img1_bytes = std::fs::read(&img1).unwrap();
        let img2_bytes = std::fs::read(&img2).unwrap();
        let img3_bytes = std::fs::read(&img3).unwrap();

        std::fs::create_dir_all(&app_data).unwrap();
        let mut manager = PostgresManager::new(&app_data);
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok);

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
            &album.display().to_string(),
            "snap_album",
        )
        .await
        .unwrap();

        let (snapshot_id, snapshot_hash) =
            capture_source_album_snapshot(&client, import_run_id, album_id, &album)
                .await
                .unwrap();

        assert_eq!(snapshot_hash.len(), 32);

        // The snapshot hash lives on source_album_snapshots.snapshot_hash
        // (single source of truth after migration 0009 dropped the
        // redundant import_albums.source_snapshot_hash column).
        let snapshot = ImportRepository::get_source_album_snapshot(&client, album_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.snapshot_hash, snapshot_hash);
        assert_eq!(snapshot.snapshot_id, snapshot_id);
        assert_eq!(snapshot.import_run_id, import_run_id);
        assert_eq!(snapshot.import_album_id, album_id);
        assert_eq!(snapshot.snapshot_hash, snapshot_hash);

        let files = ImportRepository::get_snapshot_files(&client, snapshot_id)
            .await
            .unwrap();
        assert_eq!(files.len(), 5);

        let paths: Vec<&str> = files.iter().map(|f| f.relative_path.as_str()).collect();
        assert!(
            paths.contains(&"description.txt"),
            "missing description.txt in {paths:?}"
        );
        assert!(
            paths.contains(&"nested/meta.xmp"),
            "missing nested/meta.xmp in {paths:?}"
        );
        let image_paths: Vec<&&str> = paths
            .iter()
            .filter(|p| p.ends_with(".jpg") || p.ends_with(".png") || p.ends_with(".webp"))
            .collect();
        assert_eq!(
            image_paths.len(),
            3,
            "expected 3 images in snapshot, got {image_paths:?}"
        );

        for f in &files {
            assert!(!f.relative_path.starts_with('/'));
            assert!(!f.relative_path.contains(".."));
            assert!(!f.relative_path.contains('\\'));
            assert_eq!(f.blake3.len(), 32);
        }

        let recomputed = compute_snapshot_hash(
            &files
                .iter()
                .map(|f| NewSnapshotFile {
                    relative_path: f.relative_path.clone(),
                    file_type: f.file_type.clone(),
                    file_size: f.file_size,
                    blake3: f.blake3.clone(),
                })
                .collect::<Vec<_>>(),
        );
        assert_eq!(recomputed, snapshot_hash, "snapshot_hash must be stable");

        let verify_errors = verify_source_album_snapshot(&client, album_id, &album)
            .await
            .unwrap();
        assert!(
            verify_errors.is_empty(),
            "expected no errors, got: {verify_errors:?}"
        );

        drop(client);
        db_handle.abort();
        manager.shutdown().await.unwrap();

        assert_eq!(std::fs::read(&img1).unwrap(), img1_bytes);
        assert_eq!(std::fs::read(&img2).unwrap(), img2_bytes);
        assert_eq!(std::fs::read(&img3).unwrap(), img3_bytes);
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_snapshot_verify_detects_missing_file() {
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            return;
        }

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let album = create_test_album(&source_root, "verify_album");
        create_test_image(&album, "img1.jpg");
        create_test_image(&album, "img2.png");
        std::fs::write(album.join("description.txt"), b"notes").unwrap();

        std::fs::create_dir_all(&app_data).unwrap();
        let mut manager = PostgresManager::new(&app_data);
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok);

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
            &album.display().to_string(),
            "verify_album",
        )
        .await
        .unwrap();

        capture_source_album_snapshot(&client, import_run_id, album_id, &album)
            .await
            .unwrap();

        std::fs::remove_file(album.join("description.txt")).unwrap();

        let errors = verify_source_album_snapshot(&client, album_id, &album)
            .await
            .unwrap();
        assert!(!errors.is_empty(), "should detect missing file");
        let has_missing = errors
            .iter()
            .any(|e| matches!(e, SnapshotVerifyError::MissingFile(p) if p == "description.txt"));
        assert!(has_missing, "expected MissingFile error, got: {errors:?}");

        drop(client);
        db_handle.abort();
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_snapshot_verify_detects_extra_file() {
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            return;
        }

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let album = create_test_album(&source_root, "extra_album");
        create_test_image(&album, "img1.jpg");

        std::fs::create_dir_all(&app_data).unwrap();
        let mut manager = PostgresManager::new(&app_data);
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok);

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
            &album.display().to_string(),
            "extra_album",
        )
        .await
        .unwrap();

        capture_source_album_snapshot(&client, import_run_id, album_id, &album)
            .await
            .unwrap();

        std::fs::write(album.join("extra.txt"), b"sneaked in").unwrap();

        let errors = verify_source_album_snapshot(&client, album_id, &album)
            .await
            .unwrap();
        assert!(!errors.is_empty(), "should detect extra file");
        let has_extra = errors
            .iter()
            .any(|e| matches!(e, SnapshotVerifyError::ExtraFile(p) if p == "extra.txt"));
        assert!(has_extra, "expected ExtraFile error, got: {errors:?}");

        drop(client);
        db_handle.abort();
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    #[cfg(feature = "real-db-tests")]
    async fn real_snapshot_verify_detects_hash_mismatch() {
        use crate::infrastructure::postgres::{MigrationRunner, PostgresManager};

        if std::env::var("IMAGEDB_POSTGRES_BIN")
            .unwrap_or_default()
            .is_empty()
        {
            return;
        }

        let tmp = TempDir::new().unwrap();
        let app_data = tmp.path().join("app_data");
        let source_root = tmp.path().join("source");
        let album = create_test_album(&source_root, "hash_album");
        create_test_image(&album, "img1.jpg");
        std::fs::write(album.join("description.txt"), b"original").unwrap();

        std::fs::create_dir_all(&app_data).unwrap();
        let mut manager = PostgresManager::new(&app_data);
        assert!(manager.binaries_available());
        let probe = manager.initialize().await.unwrap();
        assert!(probe.connection_ok);

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
            &album.display().to_string(),
            "hash_album",
        )
        .await
        .unwrap();

        capture_source_album_snapshot(&client, import_run_id, album_id, &album)
            .await
            .unwrap();

        std::fs::write(album.join("description.txt"), b"tampered content").unwrap();

        let errors = verify_source_album_snapshot(&client, album_id, &album)
            .await
            .unwrap();
        assert!(!errors.is_empty(), "should detect hash mismatch");
        let has_hash = errors.iter().any(|e| matches!(e, SnapshotVerifyError::HashMismatch { path } if path == "description.txt"));
        assert!(has_hash, "expected HashMismatch error, got: {errors:?}");
        let has_snapshot_mismatch = errors
            .iter()
            .any(|e| matches!(e, SnapshotVerifyError::SnapshotHashMismatch { .. }));
        assert!(
            has_snapshot_mismatch,
            "expected SnapshotHashMismatch, got: {errors:?}"
        );

        drop(client);
        db_handle.abort();
        manager.shutdown().await.unwrap();
    }
}

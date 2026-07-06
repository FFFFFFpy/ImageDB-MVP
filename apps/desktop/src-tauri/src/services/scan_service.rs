use crate::domain::import_state::{
    Decision, DecisionSource, DecodeState, DuplicateScope, ImportImageState, ImportRunState,
    MatchType, MatchingStrategy, ScanProgress, TransformType, SUPPORTED_IMAGE_EXTENSIONS,
};
use crate::error::AppError;
use crate::infrastructure::image_fingerprint::{
    fingerprint_image_with_transforms, hash_hamming_distance, TransformVariant,
};
use crate::infrastructure::postgres::PostgresManager;
use crate::infrastructure::settings::SettingsStore;
use crate::repositories::import_repository::{
    ImportRepository, LibraryImageRow, NewDuplicateCandidate, NewImportImage,
};
use crate::services::source_snapshot_service::{
    capture_source_album_snapshot, verify_source_album_snapshot,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
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

struct FingerprintedData {
    file_size: u64,
    width: u32,
    height: u32,
    format: String,
    blake3_bytes: Vec<u8>,
    pixel_hash_bytes: Vec<u8>,
    gradient_hash_bytes: Vec<u8>,
    block_hash_bytes: Vec<u8>,
    median_hash_bytes: Vec<u8>,
    blake3_hex: String,
    pixel_hash_hex: String,
    transform_variants: Vec<TransformVariant>,
}

struct AlbumImageEntry {
    album_db_id: Uuid,
    image_db_id: Uuid,
    fp: FingerprintedData,
}

struct AlbumDetectionContext<'a> {
    client: &'a Client,
    import_run_id: Uuid,
    album_id: Uuid,
    progress_tracker: &'a Mutex<ScanProgress>,
    cancelled: &'a AtomicBool,
}

struct PerceptualHex {
    gradient: String,
    block: String,
    median: String,
}

impl PerceptualHex {
    fn from_bytes(
        gradient: &Option<Vec<u8>>,
        block: &Option<Vec<u8>>,
        median: &Option<Vec<u8>>,
    ) -> Option<Self> {
        match (gradient, block, median) {
            (Some(g), Some(b), Some(m)) => Some(Self {
                gradient: bytes_to_hex(g),
                block: bytes_to_hex(b),
                median: bytes_to_hex(m),
            }),
            _ => None,
        }
    }
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

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compute perceptual band values from fingerprint data for bucketed similarity search.
/// Each band is the first 4 bytes of a perceptual hash component.
fn compute_perceptual_bands(fp: &FingerprintedData) -> Vec<Vec<u8>> {
    let mut bands = Vec::new();
    for hash_bytes in [
        &fp.gradient_hash_bytes,
        &fp.block_hash_bytes,
        &fp.median_hash_bytes,
    ] {
        if hash_bytes.len() >= 4 {
            bands.push(hash_bytes[..4].to_vec());
        } else if !hash_bytes.is_empty() {
            bands.push(hash_bytes.clone());
        }
    }
    bands
}

fn scan_directory_for_albums(source_root: &Path) -> Result<Vec<AlbumEntry>, AppError> {
    let entries = std::fs::read_dir(source_root)?;
    let mut albums = Vec::new();
    for entry in entries {
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

fn scan_album_for_images(
    album_path: &Path,
    album_name: &str,
) -> Result<Vec<ScannedImage>, AppError> {
    let mut images = Vec::new();
    walk_album_images(album_path, album_path, album_name, &mut images)?;
    images.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(images)
}

fn walk_album_images(
    dir: &Path,
    album_path: &Path,
    album_name: &str,
    images: &mut Vec<ScannedImage>,
) -> Result<(), AppError> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            walk_album_images(&path, album_path, album_name, images)?;
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

struct PerceptualEvidence {
    gradient_distance: i32,
    block_distance: i32,
    median_distance: i32,
    transform_type: TransformType,
    confidence: f64,
}

impl PerceptualEvidence {
    fn total_distance(&self) -> i32 {
        self.gradient_distance + self.block_distance + self.median_distance
    }
}

fn compare_perceptual_intra(
    a: &FingerprintedData,
    b: &FingerprintedData,
    thresholds: crate::domain::import_state::PerceptualThresholds,
) -> Option<PerceptualEvidence> {
    let max_total = thresholds.similar_max_total;
    let mut best: Option<PerceptualEvidence> = None;

    for va in &a.transform_variants {
        for vb in &b.transform_variants {
            let gd = hash_hamming_distance(&va.hashes.gradient, &vb.hashes.gradient) as i32;
            let bd = hash_hamming_distance(&va.hashes.block, &vb.hashes.block) as i32;
            let md = hash_hamming_distance(&va.hashes.median, &vb.hashes.median) as i32;
            let total = gd + bd + md;

            if total > max_total {
                continue;
            }

            let rel_transform = compose_transform(va.transform, vb.transform);
            let evidence = PerceptualEvidence {
                gradient_distance: gd,
                block_distance: bd,
                median_distance: md,
                transform_type: rel_transform,
                confidence: 1.0 - (total as f64 / 192.0),
            };

            let is_better = best
                .as_ref()
                .map(|prev| total < prev.total_distance())
                .unwrap_or(true);

            if is_better {
                best = Some(evidence);
            }
        }
    }

    best
}

fn compare_perceptual_library(
    import_fp: &FingerprintedData,
    lib_hex: &PerceptualHex,
    thresholds: crate::domain::import_state::PerceptualThresholds,
) -> Option<PerceptualEvidence> {
    let max_total = thresholds.similar_max_total;
    let mut best: Option<PerceptualEvidence> = None;

    for variant in &import_fp.transform_variants {
        let gd = hash_hamming_distance(&variant.hashes.gradient, &lib_hex.gradient) as i32;
        let bd = hash_hamming_distance(&variant.hashes.block, &lib_hex.block) as i32;
        let md = hash_hamming_distance(&variant.hashes.median, &lib_hex.median) as i32;
        let total = gd + bd + md;

        if total > max_total {
            continue;
        }

        let evidence = PerceptualEvidence {
            gradient_distance: gd,
            block_distance: bd,
            median_distance: md,
            transform_type: variant.transform,
            confidence: 1.0 - (total as f64 / 192.0),
        };

        let is_better = best
            .as_ref()
            .map(|prev| total < prev.total_distance())
            .unwrap_or(true);

        if is_better {
            best = Some(evidence);
        }
    }

    best
}

fn classify_perceptual(
    evidence: &PerceptualEvidence,
    thresholds: crate::domain::import_state::PerceptualThresholds,
) -> (MatchType, Option<Decision>, Option<DecisionSource>) {
    let max_each = evidence
        .gradient_distance
        .max(evidence.block_distance)
        .max(evidence.median_distance);
    let is_near = max_each <= thresholds.near_max_distance;

    let match_type = if is_near {
        MatchType::PerceptualNear
    } else {
        MatchType::PerceptualSimilar
    };

    let (decision, source) = if thresholds.auto_decide && is_near {
        (
            Some(Decision::AutoDuplicate),
            Some(DecisionSource::PerceptualRule),
        )
    } else {
        (None, None)
    };

    (match_type, decision, source)
}

fn compose_transform(a: TransformType, b: TransformType) -> TransformType {
    if a == b && a != TransformType::Identity {
        match a {
            TransformType::Rot90 | TransformType::Rot270 => return TransformType::Rot180,
            TransformType::Rot180 => return TransformType::Identity,
            TransformType::FlipH | TransformType::FlipV => return TransformType::Identity,
            TransformType::Transpose | TransformType::Transverse => return TransformType::Identity,
            _ => {}
        }
    }
    if b == TransformType::Identity {
        return a;
    }
    if a == TransformType::Identity {
        return b;
    }
    let m_a = transform_matrix(a);
    let m_b = transform_matrix(b);
    let m = [
        m_a[0] * m_b[0] + m_a[1] * m_b[2],
        m_a[0] * m_b[1] + m_a[1] * m_b[3],
        m_a[2] * m_b[0] + m_a[3] * m_b[2],
        m_a[2] * m_b[1] + m_a[3] * m_b[3],
    ];
    matrix_to_transform(m)
}

fn transform_matrix(t: TransformType) -> [i32; 4] {
    match t {
        TransformType::Identity => [1, 0, 0, 1],
        TransformType::Rot90 => [0, -1, 1, 0],
        TransformType::Rot180 => [-1, 0, 0, -1],
        TransformType::Rot270 => [0, 1, -1, 0],
        TransformType::FlipH => [-1, 0, 0, 1],
        TransformType::FlipV => [1, 0, 0, -1],
        TransformType::Transpose => [0, 1, 1, 0],
        TransformType::Transverse => [0, -1, -1, 0],
    }
}

fn matrix_to_transform(m: [i32; 4]) -> TransformType {
    for t in TransformType::ALL {
        if transform_matrix(t) == m {
            return t;
        }
    }
    TransformType::Identity
}

fn fingerprint_image_sync(path: &Path) -> Result<FingerprintedData, AppError> {
    let (fp, variants) = fingerprint_image_with_transforms(path)?;
    let blake3_bytes = hex_to_bytes(&fp.blake3);
    let pixel_hash_bytes = hex_to_bytes(&fp.pixel_hash);
    let gradient_hash_bytes = hex_to_bytes(&fp.gradient_hash);
    let block_hash_bytes = hex_to_bytes(&fp.block_hash);
    let median_hash_bytes = hex_to_bytes(&fp.median_hash);
    Ok(FingerprintedData {
        file_size: fp.file_size,
        width: fp.width,
        height: fp.height,
        format: fp.format,
        blake3_bytes,
        pixel_hash_bytes,
        gradient_hash_bytes,
        block_hash_bytes,
        median_hash_bytes,
        blake3_hex: fp.blake3,
        pixel_hash_hex: fp.pixel_hash,
        transform_variants: variants,
    })
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

async fn insert_candidate_and_count(
    client: &Client,
    candidate: NewDuplicateCandidate,
    duplicate_count: &mut u32,
    progress: &mut ScanProgressEvent,
    progress_tracker: &Mutex<ScanProgress>,
) -> Result<(), AppError> {
    ImportRepository::insert_duplicate_candidate(client, candidate).await?;
    *duplicate_count += 1;
    progress.duplicate_count = *duplicate_count;
    emit_progress(progress, progress_tracker).await;
    Ok(())
}

async fn detect_album_duplicates(
    ctx: &AlbumDetectionContext<'_>,
    images: &[&AlbumImageEntry],
    duplicate_count: &mut u32,
    progress: &mut ScanProgressEvent,
) -> Result<(), AppError> {
    let strategy = MatchingStrategy::Balanced;
    let thresholds = strategy.perceptual_thresholds();

    for i in 0..images.len() {
        if ctx.cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        for j in (i + 1)..images.len() {
            let a = images[i];
            let b = images[j];
            let file_exact = a.fp.file_size == b.fp.file_size && a.fp.blake3_hex == b.fp.blake3_hex;
            let pixel_exact = a.fp.pixel_hash_hex == b.fp.pixel_hash_hex;

            if file_exact {
                insert_candidate_and_count(
                    ctx.client,
                    NewDuplicateCandidate {
                        import_run_id: ctx.import_run_id,
                        source_image_id: a.image_db_id,
                        candidate_source_image_id: Some(b.image_db_id),
                        candidate_library_image_id: None,
                        scope: DuplicateScope::IntraAlbum,
                        match_type: MatchType::FileExact,
                        blake3_equal: true,
                        pixel_hash_equal: pixel_exact,
                        gradient_distance: None,
                        block_distance: None,
                        median_distance: None,
                        transform_type: None,
                        confidence: Some(1.0),
                        decision: Some(Decision::AutoDuplicate),
                        decision_source: Some(DecisionSource::ExactRule),
                    },
                    duplicate_count,
                    progress,
                    ctx.progress_tracker,
                )
                .await?;
            } else if pixel_exact {
                insert_candidate_and_count(
                    ctx.client,
                    NewDuplicateCandidate {
                        import_run_id: ctx.import_run_id,
                        source_image_id: a.image_db_id,
                        candidate_source_image_id: Some(b.image_db_id),
                        candidate_library_image_id: None,
                        scope: DuplicateScope::IntraAlbum,
                        match_type: MatchType::PixelExact,
                        blake3_equal: false,
                        pixel_hash_equal: true,
                        gradient_distance: None,
                        block_distance: None,
                        median_distance: None,
                        transform_type: None,
                        confidence: Some(1.0),
                        decision: Some(Decision::AutoDuplicate),
                        decision_source: Some(DecisionSource::ExactRule),
                    },
                    duplicate_count,
                    progress,
                    ctx.progress_tracker,
                )
                .await?;
            } else if let Some(evidence) = compare_perceptual_intra(&a.fp, &b.fp, thresholds) {
                let (match_type, decision, source) = classify_perceptual(&evidence, thresholds);
                insert_candidate_and_count(
                    ctx.client,
                    NewDuplicateCandidate {
                        import_run_id: ctx.import_run_id,
                        source_image_id: a.image_db_id,
                        candidate_source_image_id: Some(b.image_db_id),
                        candidate_library_image_id: None,
                        scope: DuplicateScope::IntraAlbum,
                        match_type,
                        blake3_equal: false,
                        pixel_hash_equal: false,
                        gradient_distance: Some(evidence.gradient_distance),
                        block_distance: Some(evidence.block_distance),
                        median_distance: Some(evidence.median_distance),
                        transform_type: Some(evidence.transform_type.to_string()),
                        confidence: Some(evidence.confidence),
                        decision,
                        decision_source: source,
                    },
                    duplicate_count,
                    progress,
                    ctx.progress_tracker,
                )
                .await?;
            }
        }
    }

    let album_blake3: Vec<Vec<u8>> = images
        .iter()
        .filter_map(|e| {
            if e.fp.blake3_bytes.is_empty() {
                None
            } else {
                Some(e.fp.blake3_bytes.clone())
            }
        })
        .collect();
    if !album_blake3.is_empty() {
        let siblings = ImportRepository::find_sibling_images_by_blake3(
            ctx.client,
            ctx.import_run_id,
            &album_blake3,
        )
        .await?;
        let mut by_hash: std::collections::HashMap<Vec<u8>, Vec<(Uuid, Uuid)>> =
            std::collections::HashMap::new();
        for (id, sibling_album_id, _file_size, b3) in siblings {
            by_hash.entry(b3).or_default().push((id, sibling_album_id));
        }
        for group in by_hash.values() {
            for i in 0..group.len() {
                for j in (i + 1)..group.len() {
                    let (a_id, a_album) = group[i];
                    let (b_id, b_album) = group[j];
                    if a_album == b_album {
                        continue;
                    }
                    if !((a_album == ctx.album_id) ^ (b_album == ctx.album_id)) {
                        continue;
                    }
                    insert_candidate_and_count(
                        ctx.client,
                        NewDuplicateCandidate {
                            import_run_id: ctx.import_run_id,
                            source_image_id: a_id,
                            candidate_source_image_id: Some(b_id),
                            candidate_library_image_id: None,
                            scope: DuplicateScope::CrossAlbum,
                            match_type: MatchType::FileExact,
                            blake3_equal: true,
                            pixel_hash_equal: false,
                            gradient_distance: None,
                            block_distance: None,
                            median_distance: None,
                            transform_type: None,
                            confidence: Some(1.0),
                            decision: Some(Decision::AutoDuplicate),
                            decision_source: Some(DecisionSource::ExactRule),
                        },
                        duplicate_count,
                        progress,
                        ctx.progress_tracker,
                    )
                    .await?;
                }
            }
        }

        let matched_library =
            ImportRepository::find_library_images_by_blake3(ctx.client, &album_blake3).await?;
        let mut blake3_to_lib: std::collections::HashMap<Vec<u8>, Vec<LibraryImageRow>> =
            std::collections::HashMap::new();
        for lib in &matched_library {
            blake3_to_lib
                .entry(lib.blake3.clone())
                .or_default()
                .push(lib.clone());
        }
        for entry in images {
            if let Some(libs) = blake3_to_lib.get(&entry.fp.blake3_bytes) {
                for lib in libs {
                    let pixel_exact = lib
                        .pixel_hash
                        .as_ref()
                        .map(|ph| *ph == entry.fp.pixel_hash_bytes)
                        .unwrap_or(false);
                    insert_candidate_and_count(
                        ctx.client,
                        NewDuplicateCandidate {
                            import_run_id: ctx.import_run_id,
                            source_image_id: entry.image_db_id,
                            candidate_source_image_id: None,
                            candidate_library_image_id: Some(lib.id),
                            scope: DuplicateScope::Library,
                            match_type: MatchType::FileExact,
                            blake3_equal: true,
                            pixel_hash_equal: pixel_exact,
                            gradient_distance: None,
                            block_distance: None,
                            median_distance: None,
                            transform_type: None,
                            confidence: Some(1.0),
                            decision: Some(Decision::AutoDuplicate),
                            decision_source: Some(DecisionSource::ExactRule),
                        },
                        duplicate_count,
                        progress,
                        ctx.progress_tracker,
                    )
                    .await?;
                }
            }
        }
    }

    let max_perceptual_candidates: usize = 50;
    for entry in images {
        if ctx.cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        let bands = compute_perceptual_bands(&entry.fp);
        let mut recalled: std::collections::HashSet<Uuid> = std::collections::HashSet::new();

        for (band_idx, band_val) in bands.iter().enumerate() {
            if band_val.is_empty() {
                continue;
            }
            let candidates = ImportRepository::find_library_images_by_perceptual_band(
                ctx.client,
                band_idx as u8,
                band_val,
                max_perceptual_candidates,
            )
            .await?;
            for lib in candidates {
                if recalled.contains(&lib.id) {
                    continue;
                }
                recalled.insert(lib.id);

                let Some(lib_hex) = PerceptualHex::from_bytes(
                    &lib.gradient_hash,
                    &lib.block_hash,
                    &lib.median_hash,
                ) else {
                    continue;
                };
                let Some(evidence) = compare_perceptual_library(&entry.fp, &lib_hex, thresholds)
                else {
                    continue;
                };
                let (match_type, decision, source) = classify_perceptual(&evidence, thresholds);
                insert_candidate_and_count(
                    ctx.client,
                    NewDuplicateCandidate {
                        import_run_id: ctx.import_run_id,
                        source_image_id: entry.image_db_id,
                        candidate_source_image_id: None,
                        candidate_library_image_id: Some(lib.id),
                        scope: DuplicateScope::Library,
                        match_type,
                        blake3_equal: false,
                        pixel_hash_equal: false,
                        gradient_distance: Some(evidence.gradient_distance),
                        block_distance: Some(evidence.block_distance),
                        median_distance: Some(evidence.median_distance),
                        transform_type: Some(evidence.transform_type.to_string()),
                        confidence: Some(evidence.confidence),
                        decision,
                        decision_source: source,
                    },
                    duplicate_count,
                    progress,
                    ctx.progress_tracker,
                )
                .await?;
            }
        }
    }

    Ok(())
}

pub async fn run_scan(
    postgres_manager: Arc<Mutex<PostgresManager>>,
    settings: Arc<Mutex<SettingsStore>>,
    source_root: String,
    cancelled: Arc<AtomicBool>,
    progress_tracker: Arc<Mutex<ScanProgress>>,
) -> Result<ScanProgress, AppError> {
    run_scan_inner(
        postgres_manager,
        settings,
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
    source_root: String,
    import_run_id: Uuid,
    cancelled: Arc<AtomicBool>,
    progress_tracker: Arc<Mutex<ScanProgress>>,
) -> Result<ScanProgress, AppError> {
    run_scan_inner(
        postgres_manager,
        settings,
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
    let mut total_images: u32 = 0;
    let mut duplicate_count: u32 = 0;

    let source_path = PathBuf::from(&source_root);

    let (client, handle) = {
        let mgr = postgres_manager.lock().await;
        mgr.connect().await?
    };

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
        client
            .query_opt(
                "SELECT id FROM import_runs
                 WHERE source_root = $1
                   AND state IN ('analyzing', 'scanning', 'fingerprinting', 'cancelled', 'failed')
                 ORDER BY started_at DESC
                 LIMIT 1",
                &[&source_root],
            )
            .await
            .map_err(|e| AppError::Internal(format!("failed to query resumable import run: {e}")))?
    };
    let import_run_id = if let Some(row) = existing_run {
        let id: Uuid = row.get("id");
        ImportRepository::mark_stale_analyzing_albums(&client, id).await?;
        ImportRepository::update_import_run_state(&client, id, &ImportRunState::Analyzing).await?;
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

    let albums = match scan_directory_for_albums(&source_path) {
        Ok(albums) => albums,
        Err(e) => {
            ImportRepository::update_import_run_error(
                &client,
                import_run_id,
                "SCAN_FAILED",
                &e.to_string(),
            )
            .await?;
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

    if albums.is_empty() {
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

    let mut all_album_images: Vec<AlbumImageEntry> = Vec::new();

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
        if let Err(e) =
            capture_source_album_snapshot(&client, import_run_id, album_db_id, &album_path).await
        {
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
        match verify_source_album_snapshot(&client, album_db_id, &album_path).await {
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
        let scanned_images = match scan_album_for_images(&album_path, &album_name) {
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

        for img in scanned_images {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }

            let fp_result = fingerprint_image_sync(&img.source_path);

            match fp_result {
                Ok(fp) => {
                    let image_id = ImportRepository::insert_import_image(
                        &client,
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
                            gradient_hash: Some(fp.gradient_hash_bytes.clone()),
                            block_hash: Some(fp.block_hash_bytes.clone()),
                            median_hash: Some(fp.median_hash_bytes.clone()),
                            fingerprint_version: Some("1".to_string()),
                            state: ImportImageState::Fingerprinted,
                        },
                    )
                    .await?;

                    all_album_images.push(AlbumImageEntry {
                        album_db_id,
                        image_db_id: image_id,
                        fp,
                    });

                    total_images += 1;
                    progress.total_images = total_images;
                    progress.processed_images = total_images;
                    emit_progress(&progress, &progress_tracker).await;
                }
                Err(e) => {
                    ImportRepository::insert_import_image(
                        &client,
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
                            gradient_hash: None,
                            block_hash: None,
                            median_hash: None,
                            fingerprint_version: None,
                            state: ImportImageState::Failed,
                        },
                    )
                    .await?;

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
                    total_images += 1;
                    progress.total_images = total_images;
                    progress.processed_images = total_images;
                    emit_progress(&progress, &progress_tracker).await;
                }
            }
        }

        if cancelled.load(Ordering::Relaxed) {
            break;
        }

        progress.current_stage = "detecting_duplicates".to_string();
        emit_progress(&progress, &progress_tracker).await;
        let album_images: Vec<&AlbumImageEntry> = all_album_images
            .iter()
            .filter(|entry| entry.album_db_id == album_db_id)
            .collect();
        let detection_ctx = AlbumDetectionContext {
            client: &client,
            import_run_id,
            album_id: album_db_id,
            progress_tracker: &progress_tracker,
            cancelled: &cancelled,
        };
        if let Err(e) = detect_album_duplicates(
            &detection_ctx,
            &album_images,
            &mut duplicate_count,
            &mut progress,
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

        let album_status =
            ImportRepository::refresh_album_workflow_summary(&client, album_db_id).await?;
        tracing::info!(
            %import_run_id,
            %album_db_id,
            album = %album_name,
            processed_images = progress.processed_images,
            album_state = %album_status.state,
            review_candidates = album_status.review_candidate_count,
            error_count = progress.error_count,
            "scan album analysis checkpoint persisted"
        );
    }

    if cancelled.load(Ordering::Relaxed) {
        ImportRepository::update_import_run_state(
            &client,
            import_run_id,
            &ImportRunState::Cancelled,
        )
        .await?;
        progress.state = "cancelled".to_string();
        progress.current_stage = "cancelled".to_string();
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
            state: "cancelled".to_string(),
            ..ScanProgress::idle()
        });
    }

    let statistics = serde_json::json!({
        "total_albums": progress.total_albums,
        "total_images": total_images,
        "duplicate_count": duplicate_count,
        "error_count": errors.len(),
    });
    ImportRepository::update_import_run_statistics(&client, import_run_id, &statistics).await?;

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
    let final_run_state = if review_progress.total > review_progress.decided {
        ImportRunState::ReviewRequired
    } else if has_failed_albums {
        ImportRunState::Failed
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
        total_images,
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
        processed_images: total_images,
        total_albums: progress.total_albums,
        total_images,
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
    use crate::domain::import_state::ImportAlbumState;
    use crate::repositories::import_repository::NewSnapshotFile;
    #[cfg(feature = "real-db-tests")]
    use crate::services::source_snapshot_service::SnapshotVerifyError;
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
    fn test_hex_to_bytes_roundtrip() {
        let hex = "deadbeef01234567";
        let bytes = hex_to_bytes(hex);
        assert_eq!(bytes.len(), 8);
        let back: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(back, hex);
    }

    #[test]
    fn test_hex_to_bytes_empty() {
        let bytes = hex_to_bytes("");
        assert!(bytes.is_empty());
    }

    #[test]
    fn test_fingerprint_image_sync() {
        let tmp = TempDir::new().unwrap();
        let path = create_test_image(tmp.path(), "test.png");
        let fp = fingerprint_image_sync(&path).unwrap();
        assert!(fp.width > 0);
        assert!(fp.height > 0);
        assert!(!fp.blake3_hex.is_empty());
        assert!(!fp.pixel_hash_hex.is_empty());
        assert!(!fp.blake3_bytes.is_empty());
        assert!(!fp.pixel_hash_bytes.is_empty());
        assert!(!fp.gradient_hash_bytes.is_empty());
        assert!(!fp.block_hash_bytes.is_empty());
        assert!(!fp.median_hash_bytes.is_empty());
        assert_eq!(fp.transform_variants.len(), 8);
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
            fp1.blake3_hex, fp2.blake3_hex,
            "BLAKE3 should match for exact copy"
        );
        assert_eq!(
            fp1.pixel_hash_hex, fp2.pixel_hash_hex,
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
            fp1.blake3_hex, fp2.blake3_hex,
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
            fp_png.blake3_hex, fp_jpg.blake3_hex,
            "different formats should have different BLAKE3"
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
    fn test_strategy_determinism() {
        let t1 = MatchingStrategy::Balanced.perceptual_thresholds();
        let t2 = MatchingStrategy::Balanced.perceptual_thresholds();
        assert_eq!(t1.near_max_distance, t2.near_max_distance);
        assert_eq!(t1.similar_max_total, t2.similar_max_total);
        assert_eq!(t1.auto_decide, t2.auto_decide);
    }

    #[test]
    fn test_classify_perceptual_near_auto() {
        let thresholds = MatchingStrategy::Strict.perceptual_thresholds();
        let evidence = PerceptualEvidence {
            gradient_distance: 2,
            block_distance: 1,
            median_distance: 2,
            transform_type: TransformType::Identity,
            confidence: 0.95,
        };
        let (mt, dec, src) = classify_perceptual(&evidence, thresholds);
        assert_eq!(mt, MatchType::PerceptualNear);
        assert_eq!(dec, Some(Decision::AutoDuplicate));
        assert_eq!(src, Some(DecisionSource::PerceptualRule));
    }

    #[test]
    fn test_classify_perceptual_loose_review() {
        let thresholds = MatchingStrategy::Loose.perceptual_thresholds();
        let evidence = PerceptualEvidence {
            gradient_distance: 5,
            block_distance: 5,
            median_distance: 5,
            transform_type: TransformType::Identity,
            confidence: 0.8,
        };
        let (mt, dec, src) = classify_perceptual(&evidence, thresholds);
        assert_eq!(mt, MatchType::PerceptualNear);
        assert_eq!(dec, None);
        assert_eq!(src, None);
    }

    #[test]
    fn test_classify_perceptual_similar() {
        let thresholds = MatchingStrategy::Balanced.perceptual_thresholds();
        let evidence = PerceptualEvidence {
            gradient_distance: 10,
            block_distance: 6,
            median_distance: 7,
            transform_type: TransformType::Rot90,
            confidence: 0.7,
        };
        let (mt, dec, src) = classify_perceptual(&evidence, thresholds);
        assert_eq!(mt, MatchType::PerceptualSimilar);
        assert_eq!(dec, None);
        assert_eq!(src, None);
    }

    #[test]
    fn test_compare_perceptual_intra_identical() {
        let tmp = TempDir::new().unwrap();
        let path = create_test_image(tmp.path(), "img.png");
        let fp1 = fingerprint_image_sync(&path).unwrap();
        let fp2 = fingerprint_image_sync(&path).unwrap();
        let thresholds = MatchingStrategy::Strict.perceptual_thresholds();
        let evidence = compare_perceptual_intra(&fp1, &fp2, thresholds);
        assert!(evidence.is_some());
        let ev = evidence.unwrap();
        assert_eq!(ev.gradient_distance, 0);
        assert_eq!(ev.block_distance, 0);
        assert_eq!(ev.median_distance, 0);
    }

    #[test]
    fn test_compare_perceptual_intra_different() {
        let tmp = TempDir::new().unwrap();

        let mut img1 = image::RgbImage::new(64, 64);
        for y in 0..64u32 {
            for x in 0..64u32 {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                img1.put_pixel(x, y, image::Rgb([v, v, v]));
            }
        }
        let p1 = tmp.path().join("checker.png");
        img1.save(&p1).unwrap();

        let mut img2 = image::RgbImage::new(64, 64);
        for y in 0..64u32 {
            for x in 0..64u32 {
                let v = if x < 32 { 200 } else { 50 };
                img2.put_pixel(x, y, image::Rgb([v, v, v]));
            }
        }
        let p2 = tmp.path().join("split.png");
        img2.save(&p2).unwrap();

        let fp1 = fingerprint_image_sync(&p1).unwrap();
        let fp2 = fingerprint_image_sync(&p2).unwrap();
        let thresholds = MatchingStrategy::Strict.perceptual_thresholds();
        let evidence = compare_perceptual_intra(&fp1, &fp2, thresholds);
        assert!(
            evidence.is_none(),
            "different images should not match under strict thresholds"
        );
    }

    #[test]
    fn test_compose_transform_identity() {
        for t in TransformType::ALL {
            assert_eq!(compose_transform(t, TransformType::Identity), t);
            assert_eq!(compose_transform(TransformType::Identity, t), t);
        }
    }

    #[test]
    fn test_compose_transform_inverse() {
        assert_eq!(
            compose_transform(TransformType::FlipH, TransformType::FlipH),
            TransformType::Identity
        );
        assert_eq!(
            compose_transform(TransformType::Rot180, TransformType::Rot180),
            TransformType::Identity
        );
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
        assert_ne!(fp_original.blake3_hex, fp_metadata.blake3_hex);
        assert_eq!(fp_original.pixel_hash_hex, fp_metadata.pixel_hash_hex);

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
                    gradient_hash: Some(fp.gradient_hash_bytes.clone()),
                    block_hash: Some(fp.block_hash_bytes.clone()),
                    median_hash: Some(fp.median_hash_bytes.clone()),
                    fingerprint_version: Some("1".to_string()),
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
                    a.fp.file_size == b.fp.file_size && a.fp.blake3_hex == b.fp.blake3_hex;
                let pixel_exact = a.fp.pixel_hash_hex == b.fp.pixel_hash_hex;

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
                            gradient_distance: None,
                            block_distance: None,
                            median_distance: None,
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
                            gradient_distance: None,
                            block_distance: None,
                            median_distance: None,
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
                    gradient_hash: Some(fp.gradient_hash_bytes),
                    block_hash: Some(fp.block_hash_bytes),
                    median_hash: Some(fp.median_hash_bytes),
                    fingerprint_version: Some("1".to_string()),
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

        ImportRepository::insert_duplicate_candidate(
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
                gradient_distance: Some(8),
                block_distance: Some(8),
                median_distance: Some(8),
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
                gradient_distance: None,
                block_distance: None,
                median_distance: None,
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
                gradient_distance: None,
                block_distance: None,
                median_distance: None,
                transform_type: None,
                confidence: Some(1.0),
                decision: Some(Decision::AutoDuplicate),
                decision_source: Some(DecisionSource::ExactRule),
            },
        )
        .await
        .unwrap();

        let done_status = ImportRepository::refresh_album_workflow_summary(&client, done_album_id)
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
        ImportRepository::mark_import_album_failed(
            &client,
            failed_album_id,
            "TEST_FAILURE",
            "simulated album failure",
        )
        .await
        .unwrap();

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

        drop(client);
        db_handle.abort();
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

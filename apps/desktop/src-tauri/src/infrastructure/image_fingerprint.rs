use crate::domain::import_state::TransformType;
use crate::error::AppError;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageFormat};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct ImageFingerprint {
    pub fingerprint_version: u32,
    pub file_path: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub blake3: String,
    pub pixel_hash: String,
    pub gradient_hash: String,
    pub block_hash: String,
    pub median_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageFingerprintProbeResult {
    pub fingerprints: Vec<ImageFingerprint>,
    pub diagnostics: Vec<String>,
    pub success: bool,
}

#[derive(Debug, Clone)]
pub struct PerceptualHashes {
    pub gradient: String,
    pub block: String,
    pub median: String,
}

impl PerceptualHashes {
    #[allow(dead_code)]
    pub fn to_bytes(&self) -> PerceptualHashBytes {
        PerceptualHashBytes {
            gradient: hex_to_bytes(&self.gradient),
            block: hex_to_bytes(&self.block),
            median: hex_to_bytes(&self.median),
        }
    }
}

#[allow(dead_code)]
pub struct PerceptualHashBytes {
    pub gradient: Vec<u8>,
    pub block: Vec<u8>,
    pub median: Vec<u8>,
}

pub struct TransformVariant {
    pub transform: TransformType,
    pub hashes: PerceptualHashes,
}

const HASH_SIZE: u32 = 8;
const FINGERPRINT_VERSION: u32 = 1;

pub fn fingerprint_image(path: &Path) -> Result<ImageFingerprint, AppError> {
    let file_bytes = std::fs::read(path)?;
    let file_size = file_bytes.len() as u64;

    let blake3_hash = compute_blake3(&file_bytes);

    let img = image::load_from_memory(&file_bytes)?;
    let (width, height) = img.dimensions();

    let format = detect_format(&file_bytes);

    let oriented = apply_orientation(&img);
    let rgba = oriented.to_rgba8();
    let pixel_hash = compute_pixel_hash(&rgba);

    let gray = oriented.to_luma8();
    let resized = image::imageops::resize(&gray, HASH_SIZE, HASH_SIZE, FilterType::Lanczos3);

    let gradient_hash = compute_gradient_hash(&resized);
    let block_hash = compute_block_hash(&gray, 8);
    let median_hash = compute_median_hash(&resized);

    Ok(ImageFingerprint {
        fingerprint_version: FINGERPRINT_VERSION,
        file_path: path.display().to_string(),
        format,
        width,
        height,
        file_size,
        blake3: blake3_hash,
        pixel_hash,
        gradient_hash,
        median_hash,
        block_hash,
    })
}

pub fn fingerprint_image_with_transforms(
    path: &Path,
) -> Result<(ImageFingerprint, Vec<TransformVariant>), AppError> {
    let file_bytes = std::fs::read(path)?;
    let file_size = file_bytes.len() as u64;

    let blake3_hash = compute_blake3(&file_bytes);
    let img = image::load_from_memory(&file_bytes)?;
    let (width, height) = img.dimensions();
    let format = detect_format(&file_bytes);

    let oriented = apply_orientation(&img);
    let rgba = oriented.to_rgba8();
    let pixel_hash = compute_pixel_hash(&rgba);

    let gray = oriented.to_luma8();
    let resized = image::imageops::resize(&gray, HASH_SIZE, HASH_SIZE, FilterType::Lanczos3);

    let gradient_hash = compute_gradient_hash(&resized);
    let block_hash = compute_block_hash(&gray, 8);
    let median_hash = compute_median_hash(&resized);

    let fp = ImageFingerprint {
        fingerprint_version: FINGERPRINT_VERSION,
        file_path: path.display().to_string(),
        format,
        width,
        height,
        file_size,
        blake3: blake3_hash,
        pixel_hash,
        gradient_hash,
        median_hash,
        block_hash,
    };

    let variants = compute_transform_variants(&resized);

    Ok((fp, variants))
}

pub fn compute_transform_variants(small_gray_8x8: &image::GrayImage) -> Vec<TransformVariant> {
    TransformType::ALL
        .iter()
        .map(|&transform| {
            let transformed = transform_gray_8x8(small_gray_8x8, transform);
            let hashes = compute_perceptual_hashes_8x8(&transformed);
            TransformVariant { transform, hashes }
        })
        .collect()
}

pub fn compute_perceptual_hashes_8x8(small_gray: &image::GrayImage) -> PerceptualHashes {
    let gradient = compute_gradient_hash(small_gray);
    let median = compute_median_hash(small_gray);
    let upscaled = image::imageops::resize(small_gray, 64, 64, FilterType::Nearest);
    let block = compute_block_hash(&upscaled, 8);
    PerceptualHashes {
        gradient,
        block,
        median,
    }
}

pub fn hash_hamming_distance(a: &str, b: &str) -> u32 {
    let bytes_a = hex_to_bytes(a);
    let bytes_b = hex_to_bytes(b);
    let min_len = bytes_a.len().min(bytes_b.len());
    let mut distance: u32 = 0;
    for i in 0..min_len {
        distance += (bytes_a[i] ^ bytes_b[i]).count_ones();
    }
    distance += ((bytes_a.len() as i64 - bytes_b.len() as i64).unsigned_abs() as u32) * 8;
    distance
}

fn transform_gray_8x8(img: &image::GrayImage, transform: TransformType) -> image::GrayImage {
    let size = 8u32;
    let mut out = image::GrayImage::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let (sx, sy) = match transform {
                TransformType::Identity => (x, y),
                TransformType::Rot90 => (y, size - 1 - x),
                TransformType::Rot180 => (size - 1 - x, size - 1 - y),
                TransformType::Rot270 => (size - 1 - y, x),
                TransformType::FlipH => (size - 1 - x, y),
                TransformType::FlipV => (x, size - 1 - y),
                TransformType::Transpose => (y, x),
                TransformType::Transverse => (size - 1 - y, size - 1 - x),
            };
            out.put_pixel(x, y, *img.get_pixel(sx, sy));
        }
    }
    out
}

fn detect_format(bytes: &[u8]) -> String {
    match image::guess_format(bytes) {
        Ok(ImageFormat::Jpeg) => "JPEG".to_string(),
        Ok(ImageFormat::Png) => "PNG".to_string(),
        Ok(ImageFormat::WebP) => "WebP".to_string(),
        Ok(f) => format!("{f:?}"),
        Err(_) => "unknown".to_string(),
    }
}

fn apply_orientation(img: &DynamicImage) -> DynamicImage {
    img.clone()
}

fn compute_blake3(data: &[u8]) -> String {
    let hash = blake3::hash(data);
    hash.to_hex().to_string()
}

fn compute_pixel_hash(rgba: &image::RgbaImage) -> String {
    let mut normalized: Vec<u8> = Vec::with_capacity(rgba.as_raw().len());
    for pixel in rgba.pixels() {
        let [r, g, b, a] = pixel.0;
        if a == 0 {
            normalized.extend_from_slice(&[0, 0, 0, 255]);
        } else {
            normalized.extend_from_slice(&[r, g, b, 255]);
        }
    }

    let width_bytes = rgba.width() as usize * 4;
    let height = rgba.height() as usize;
    let mut row_data = Vec::with_capacity(normalized.len());
    for y in 0..height {
        let start = y * width_bytes;
        let end = start + width_bytes;
        row_data.extend_from_slice(&normalized[start..end]);
    }

    let mut versioned = Vec::with_capacity(8 + row_data.len());
    versioned.extend_from_slice(&FINGERPRINT_VERSION.to_le_bytes());
    versioned.extend_from_slice(&rgba.width().to_le_bytes());
    versioned.extend_from_slice(&row_data);

    let hash = blake3::hash(&versioned);
    let bytes = hash.as_bytes();
    hex::encode(&bytes[..8])
}

fn compute_gradient_hash(small_gray: &image::GrayImage) -> String {
    let w = small_gray.width();
    let h = small_gray.height();
    let mut bits: Vec<bool> = Vec::new();

    for y in 0..h {
        for x in 0..(w - 1) {
            let left = small_gray.get_pixel(x, y).0[0];
            let right = small_gray.get_pixel(x + 1, y).0[0];
            bits.push(left > right);
        }
    }

    bits_to_hex(&bits)
}

fn compute_block_hash(gray: &image::GrayImage, grid: u32) -> String {
    let w = gray.width();
    let h = gray.height();
    let block_w = w / grid;
    let block_h = h / grid;

    if block_w == 0 || block_h == 0 {
        return "0".repeat((grid * grid / 4) as usize);
    }

    let mut block_means = Vec::with_capacity((grid * grid) as usize);
    let mut total_sum: f64 = 0.0;
    let mut total_count: f64 = 0.0;

    for by in 0..grid {
        for bx in 0..grid {
            let x0 = bx * block_w;
            let y0 = by * block_h;
            let x1 = if bx == grid - 1 { w } else { x0 + block_w };
            let y1 = if by == grid - 1 { h } else { y0 + block_h };

            let mut sum: f64 = 0.0;
            let mut count: f64 = 0.0;
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += gray.get_pixel(x, y).0[0] as f64;
                    count += 1.0;
                }
            }
            let mean = if count > 0.0 { sum / count } else { 0.0 };
            block_means.push(mean);
            total_sum += sum;
            total_count += count;
        }
    }

    let overall_mean = if total_count > 0.0 {
        total_sum / total_count
    } else {
        0.0
    };

    let bits: Vec<bool> = block_means.iter().map(|&m| m >= overall_mean).collect();
    bits_to_hex(&bits)
}

fn compute_median_hash(small_gray: &image::GrayImage) -> String {
    let pixels: Vec<u8> = small_gray.pixels().map(|p| p.0[0]).collect();
    let median = compute_median(&pixels);
    let bits: Vec<bool> = pixels.iter().map(|&p| p >= median).collect();
    bits_to_hex(&bits)
}

fn compute_median(values: &[u8]) -> u8 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        ((sorted[mid - 1] as u16 + sorted[mid] as u16) / 2) as u8
    } else {
        sorted[mid]
    }
}

fn bits_to_hex(bits: &[bool]) -> String {
    let mut hex = String::new();
    for chunk in bits.chunks(4) {
        let mut nibble: u8 = 0;
        for (i, &bit) in chunk.iter().enumerate() {
            if bit {
                nibble |= 1 << (3 - i);
            }
        }
        hex.push(char::from_digit(nibble as u32, 16).unwrap_or('0'));
    }
    hex
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

pub fn run_probe(fixture_dir: &Path) -> ImageFingerprintProbeResult {
    let mut diagnostics = Vec::new();
    let mut fingerprints = Vec::new();

    if !fixture_dir.exists() {
        diagnostics.push(format!(
            "Fixture directory not found: {}",
            fixture_dir.display()
        ));
        return ImageFingerprintProbeResult {
            fingerprints,
            diagnostics,
            success: false,
        };
    }

    let entries: Vec<_> = match std::fs::read_dir(fixture_dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(e) => {
            diagnostics.push(format!("Cannot read fixture directory: {e}"));
            return ImageFingerprintProbeResult {
                fingerprints,
                diagnostics,
                success: false,
            };
        }
    };

    let mut found_jpeg = false;
    let mut found_png = false;
    let mut found_webp = false;

    for entry in &entries {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "jpg" | "jpeg" => found_jpeg = true,
            "png" => found_png = true,
            "webp" => found_webp = true,
            _ => continue,
        }

        match fingerprint_image(&path) {
            Ok(fp) => {
                diagnostics.push(format!(
                    "Fingerprinted {}: {} ({}x{}, {})",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    &fp.blake3[..16],
                    fp.width,
                    fp.height,
                    fp.format,
                ));
                fingerprints.push(fp);
            }
            Err(e) => {
                diagnostics.push(format!("Failed to fingerprint {}: {e}", path.display()));
            }
        }
    }

    if !found_jpeg {
        diagnostics.push("No JPEG sample found".to_string());
    }
    if !found_png {
        diagnostics.push("No PNG sample found".to_string());
    }
    if !found_webp {
        diagnostics.push("No WebP sample found".to_string());
    }

    let success = !fingerprints.is_empty();
    if success {
        diagnostics.push(format!(
            "Successfully fingerprinted {} images (JPEG:{found_jpeg}, PNG:{found_png}, WebP:{found_webp})",
            fingerprints.len()
        ));
    }

    ImageFingerprintProbeResult {
        fingerprints,
        diagnostics,
        success,
    }
}

pub fn generate_test_samples(dir: &Path) -> Result<Vec<String>, AppError> {
    let mut created = Vec::new();

    let mut img = image::RgbImage::new(64, 64);
    for y in 0..64u32 {
        for x in 0..64u32 {
            let r = ((x * 4) % 256) as u8;
            let g = ((y * 4) % 256) as u8;
            let b = (((x + y) * 2) % 256) as u8;
            img.put_pixel(x, y, image::Rgb([r, g, b]));
        }
    }

    let png_path = dir.join("test-sample.png");
    img.save(&png_path)?;
    created.push(format!("PNG: {}", png_path.display()));

    let jpeg_path = dir.join("test-sample.jpg");
    img.save_with_format(&jpeg_path, ImageFormat::Jpeg)?;
    created.push(format!("JPEG: {}", jpeg_path.display()));

    let webp_path = dir.join("test-sample.webp");
    match img.save_with_format(&webp_path, ImageFormat::WebP) {
        Ok(_) => created.push(format!("WebP: {}", webp_path.display())),
        Err(e) => created.push(format!("WebP generation skipped: {e}")),
    }

    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_test_image() -> image::RgbImage {
        let mut img = image::RgbImage::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                img.put_pixel(
                    x,
                    y,
                    image::Rgb([(x * 8) as u8, (y * 8) as u8, ((x + y) * 4) as u8]),
                );
            }
        }
        img
    }

    #[test]
    fn test_blake3_deterministic() {
        let data = b"test image data";
        let h1 = compute_blake3(data);
        let h2 = compute_blake3(data);
        assert_eq!(h1, h2);
        assert!(!h1.is_empty());
    }

    #[test]
    fn test_pixel_hash_deterministic() {
        let img = make_test_image();
        let rgba = DynamicImage::ImageRgb8(img).to_rgba8();
        let h1 = compute_pixel_hash(&rgba);
        let h2 = compute_pixel_hash(&rgba);
        assert_eq!(h1, h2);
        assert!(!h1.is_empty());
    }

    #[test]
    fn test_median_hash_deterministic() {
        let img = make_test_image();
        let gray = DynamicImage::ImageRgb8(img).to_luma8();
        let small = image::imageops::resize(&gray, HASH_SIZE, HASH_SIZE, FilterType::Lanczos3);
        let h1 = compute_median_hash(&small);
        let h2 = compute_median_hash(&small);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_block_hash_deterministic() {
        let img = make_test_image();
        let gray = DynamicImage::ImageRgb8(img).to_luma8();
        let h1 = compute_block_hash(&gray, 8);
        let h2 = compute_block_hash(&gray, 8);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_gradient_hash_deterministic() {
        let img = make_test_image();
        let gray = DynamicImage::ImageRgb8(img).to_luma8();
        let small = image::imageops::resize(&gray, HASH_SIZE, HASH_SIZE, FilterType::Lanczos3);
        let h1 = compute_gradient_hash(&small);
        let h2 = compute_gradient_hash(&small);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_generate_and_fingerprint() {
        let tmp = TempDir::new().unwrap();
        let samples = generate_test_samples(tmp.path()).unwrap();
        assert!(!samples.is_empty());

        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ["png", "jpg", "jpeg", "webp"].contains(&ext.to_str().unwrap_or("")))
                    .unwrap_or(false)
            })
            .collect();

        assert!(
            entries.len() >= 2,
            "Expected at least PNG and JPEG samples, got {}",
            entries.len()
        );

        for entry in &entries {
            let fp = fingerprint_image(&entry.path()).unwrap();
            assert!(!fp.blake3.is_empty());
            assert!(!fp.pixel_hash.is_empty());
            assert!(!fp.median_hash.is_empty());
            assert!(!fp.block_hash.is_empty());
            assert!(!fp.gradient_hash.is_empty());
            assert!(fp.width > 0);
            assert!(fp.height > 0);
        }
    }

    #[test]
    fn test_different_images_different_hashes() {
        let tmp = TempDir::new().unwrap();

        let mut img1 = image::RgbImage::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                img1.put_pixel(x, y, image::Rgb([v, v, v]));
            }
        }

        let mut img2 = image::RgbImage::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let v = if x < 16 { 200 } else { 50 };
                img2.put_pixel(x, y, image::Rgb([v, v, v]));
            }
        }

        let p1 = tmp.path().join("checker.png");
        let p2 = tmp.path().join("split.png");
        img1.save(&p1).unwrap();
        img2.save(&p2).unwrap();

        let fp1 = fingerprint_image(&p1).unwrap();
        let fp2 = fingerprint_image(&p2).unwrap();

        assert_ne!(fp1.blake3, fp2.blake3);
        assert_ne!(fp1.gradient_hash, fp2.gradient_hash);
    }

    #[test]
    fn test_run_probe() {
        let tmp = TempDir::new().unwrap();
        generate_test_samples(tmp.path()).unwrap();
        let result = run_probe(tmp.path());
        assert!(result.success);
        assert!(!result.fingerprints.is_empty());
    }

    #[test]
    fn test_hamming_distance_identical() {
        assert_eq!(hash_hamming_distance("deadbeef", "deadbeef"), 0);
        assert_eq!(hash_hamming_distance("0000", "0000"), 0);
        assert_eq!(hash_hamming_distance("ffff", "ffff"), 0);
    }

    #[test]
    fn test_hamming_distance_single_bit() {
        assert_eq!(hash_hamming_distance("0000", "0001"), 1);
        assert_eq!(hash_hamming_distance("0000", "8000"), 1);
    }

    #[test]
    fn test_hamming_distance_all_different() {
        assert_eq!(hash_hamming_distance("0000", "ffff"), 16);
    }

    #[test]
    fn test_hamming_distance_symmetric() {
        let a = "abcdef01";
        let b = "12345678";
        assert_eq!(hash_hamming_distance(a, b), hash_hamming_distance(b, a));
    }

    #[test]
    fn test_transform_variants_count() {
        let img = make_test_image();
        let gray = DynamicImage::ImageRgb8(img).to_luma8();
        let small = image::imageops::resize(&gray, HASH_SIZE, HASH_SIZE, FilterType::Lanczos3);
        let variants = compute_transform_variants(&small);
        assert_eq!(variants.len(), 8);
    }

    #[test]
    fn test_transform_identity_matches_canonical() {
        let img = make_test_image();
        let gray = DynamicImage::ImageRgb8(img).to_luma8();
        let small = image::imageops::resize(&gray, HASH_SIZE, HASH_SIZE, FilterType::Lanczos3);
        let variants = compute_transform_variants(&small);
        let identity = &variants[0];
        assert_eq!(
            identity.transform,
            crate::domain::import_state::TransformType::Identity
        );
        let canonical = compute_perceptual_hashes_8x8(&small);
        assert_eq!(identity.hashes.gradient, canonical.gradient);
        assert_eq!(identity.hashes.block, canonical.block);
        assert_eq!(identity.hashes.median, canonical.median);
    }

    #[test]
    fn test_transform_rot180_double_rot90() {
        let img = make_test_image();
        let gray = DynamicImage::ImageRgb8(img).to_luma8();
        let small = image::imageops::resize(&gray, HASH_SIZE, HASH_SIZE, FilterType::Lanczos3);

        let rot90 = transform_gray_8x8(&small, crate::domain::import_state::TransformType::Rot90);
        let rot180_via_double90 =
            transform_gray_8x8(&rot90, crate::domain::import_state::TransformType::Rot90);
        let rot180 = transform_gray_8x8(&small, crate::domain::import_state::TransformType::Rot180);

        for y in 0..8u32 {
            for x in 0..8u32 {
                assert_eq!(
                    rot180_via_double90.get_pixel(x, y).0[0],
                    rot180.get_pixel(x, y).0[0]
                );
            }
        }
    }

    #[test]
    fn test_perceptual_hashes_deterministic() {
        let img = make_test_image();
        let gray = DynamicImage::ImageRgb8(img).to_luma8();
        let small = image::imageops::resize(&gray, HASH_SIZE, HASH_SIZE, FilterType::Lanczos3);
        let h1 = compute_perceptual_hashes_8x8(&small);
        let h2 = compute_perceptual_hashes_8x8(&small);
        assert_eq!(h1.gradient, h2.gradient);
        assert_eq!(h1.block, h2.block);
        assert_eq!(h1.median, h2.median);
    }

    #[test]
    fn test_fingerprint_with_transforms() {
        let tmp = TempDir::new().unwrap();
        generate_test_samples(tmp.path()).unwrap();
        let path = tmp.path().join("test-sample.png");
        let (fp, variants) = fingerprint_image_with_transforms(&path).unwrap();
        assert!(!fp.gradient_hash.is_empty());
        assert!(!fp.block_hash.is_empty());
        assert!(!fp.median_hash.is_empty());
        assert_eq!(variants.len(), 8);
        assert_eq!(variants[0].hashes.gradient, fp.gradient_hash);
    }

    #[test]
    fn test_scaled_image_perceptual_similarity() {
        let tmp = TempDir::new().unwrap();
        let original = image::RgbImage::from_fn(64, 64, |x, y| {
            image::Rgb([
                ((x * 4) % 256) as u8,
                ((y * 4) % 256) as u8,
                (((x + y) * 2) % 256) as u8,
            ])
        });
        let p1 = tmp.path().join("original.png");
        original.save(&p1).unwrap();

        let scaled = image::imageops::resize(&original, 128, 128, FilterType::Lanczos3);
        let p2 = tmp.path().join("scaled.png");
        scaled.save(&p2).unwrap();

        let fp1 = fingerprint_image(&p1).unwrap();
        let fp2 = fingerprint_image(&p2).unwrap();

        let grad_dist = hash_hamming_distance(&fp1.gradient_hash, &fp2.gradient_hash);
        let block_dist = hash_hamming_distance(&fp1.block_hash, &fp2.block_hash);
        let median_dist = hash_hamming_distance(&fp1.median_hash, &fp2.median_hash);

        assert!(
            grad_dist + block_dist + median_dist < 30,
            "scaled image should be perceptually similar: grad={grad_dist} block={block_dist} median={median_dist}"
        );
    }

    #[test]
    fn test_mirrored_image_recallable_via_transforms() {
        let tmp = TempDir::new().unwrap();
        let img = image::RgbImage::from_fn(64, 64, |x, y| {
            image::Rgb([
                ((x * 4) % 256) as u8,
                ((y * 4) % 256) as u8,
                (((x + y) * 2) % 256) as u8,
            ])
        });
        let p1 = tmp.path().join("original.png");
        img.save(&p1).unwrap();

        let flipped = image::imageops::flip_horizontal(&img);
        let p2 = tmp.path().join("flipped.png");
        flipped.save(&p2).unwrap();

        let (_, variants1) = fingerprint_image_with_transforms(&p1).unwrap();
        let (_, variants2) = fingerprint_image_with_transforms(&p2).unwrap();

        let mut best_total = u32::MAX;
        for v1 in &variants1 {
            for v2 in &variants2 {
                let g = hash_hamming_distance(&v1.hashes.gradient, &v2.hashes.gradient);
                let b = hash_hamming_distance(&v1.hashes.block, &v2.hashes.block);
                let m = hash_hamming_distance(&v1.hashes.median, &v2.hashes.median);
                best_total = best_total.min(g + b + m);
            }
        }

        assert!(
            best_total < 20,
            "mirrored image should be recallable via transforms, best_total={best_total}"
        );
    }
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

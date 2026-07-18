use crate::domain::import_state::TransformType;
use crate::error::AppError;
use exif::{In, Reader, Tag};
use image::imageops::FilterType as ImageFilterType;
use image::{DynamicImage, GrayImage, ImageFormat, ImageReader, Limits, RgbaImage};
use image_hasher::{FilterType as HashFilterType, HashAlg, HasherConfig};
use serde::Serialize;
use std::io::Cursor;
use std::path::Path;

pub const FINGERPRINT_VERSION: u32 = 2;
pub const BLOCK_HASH_SIZE: u32 = 16;
pub const DOUBLE_GRADIENT_HASH_SIZE: u32 = 32;

pub const BLOCK_RECALL_DISTANCE_RATIO: f64 = 0.12;
pub const BLOCK_AUTO_DISTANCE_RATIO: f64 = 0.04;
pub const DOUBLE_GRADIENT_AUTO_DISTANCE_RATIO: f64 = 0.04;
pub const BLOCK_REVIEW_DISTANCE_RATIO: f64 = 0.12;
pub const DOUBLE_GRADIENT_REVIEW_DISTANCE_RATIO: f64 = 0.08;
pub const BLOCK_DISTANCE_WEIGHT: f64 = 0.40;
pub const DOUBLE_GRADIENT_DISTANCE_WEIGHT: f64 = 0.60;
pub const MAX_RECALL_CANDIDATES_PER_IMAGE: usize = 256;
pub const LARGE_IMAGE_PIXEL_THRESHOLD: u64 = 100_000_000;
pub const MAX_DECODED_IMAGE_PIXELS: u64 = 500_000_000;

const MIN_GRAYSCALE_STDDEV: f64 = 8.0;
const MIN_EFFECTIVE_GRAYSCALE_BINS: usize = 4;
const MIN_MEAN_EDGE_DELTA: f64 = 2.0;

pub const BLOCK_HASH_BIT_LENGTH: u32 = BLOCK_HASH_SIZE * BLOCK_HASH_SIZE;
// image_hasher's DoubleGradient 32x32 configuration compares both axes on
// a 17x17 working image: 16x17 horizontal plus 17x16 vertical comparisons.
pub const DOUBLE_GRADIENT_HASH_BIT_LENGTH: u32 =
    2 * (DOUBLE_GRADIENT_HASH_SIZE / 2) * (DOUBLE_GRADIENT_HASH_SIZE / 2 + 1);

#[derive(Debug, Clone, PartialEq)]
pub struct HashDistance {
    pub raw_distance: u32,
    pub normalized_distance: f64,
    pub bit_length: u32,
}

#[derive(Debug, Clone)]
pub struct BlockHashVariant {
    pub transform: TransformType,
    pub hash: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ImageFingerprintV2 {
    pub file_path: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub blake3: Vec<u8>,
    pub pixel_hash: Vec<u8>,
    pub block_hash_16: Vec<u8>,
    pub double_gradient_hash_32: Vec<u8>,
    pub perceptual_eligible: bool,
    pub block_variants: Vec<BlockHashVariant>,
    /// Small, post-orientation grayscale image retained only while the scan
    /// is active so the winning transform can be fine-hashed once.
    pub fine_thumbnail_32: GrayImage,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageFingerprintProbeEntry {
    pub fingerprint_version: u32,
    pub file_path: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub blake3_bytes: usize,
    pub pixel_hash_bytes: usize,
    pub block_hash_bits: usize,
    pub double_gradient_hash_bits: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageFingerprintProbeResult {
    pub fingerprints: Vec<ImageFingerprintProbeEntry>,
    pub diagnostics: Vec<String>,
    pub success: bool,
}

pub fn fingerprint_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2)
        .clamp(2, 8)
}

pub fn recall_radius(bit_length: u32) -> u32 {
    ((bit_length as f64) * BLOCK_RECALL_DISTANCE_RATIO).ceil() as u32
}

pub fn hamming_distance(a: &[u8], b: &[u8]) -> Result<HashDistance, AppError> {
    if a.len() != b.len() {
        return Err(AppError::Internal(format!(
            "cannot compare perceptual hashes with different lengths: {} bytes vs {} bytes",
            a.len(),
            b.len()
        )));
    }
    if a.is_empty() {
        return Err(AppError::Internal(
            "cannot compare empty perceptual hashes".to_string(),
        ));
    }

    let raw_distance = a
        .iter()
        .zip(b)
        .map(|(left, right)| (left ^ right).count_ones())
        .sum();
    let bit_length = (a.len() * 8) as u32;
    Ok(HashDistance {
        raw_distance,
        normalized_distance: raw_distance as f64 / bit_length as f64,
        bit_length,
    })
}

pub fn weighted_similarity(block_ratio: f64, double_gradient_ratio: f64) -> f64 {
    let weighted_distance = block_ratio * BLOCK_DISTANCE_WEIGHT
        + double_gradient_ratio * DOUBLE_GRADIENT_DISTANCE_WEIGHT;
    (1.0 - weighted_distance).clamp(0.0, 1.0)
}

pub fn fingerprint_image(path: &Path) -> Result<ImageFingerprintV2, AppError> {
    let file_bytes = std::fs::read(path)?;
    fingerprint_bytes(path, &file_bytes)
}

pub fn inspect_image_dimensions(path: &Path) -> Result<(u32, u32, u64), AppError> {
    let reader = ImageReader::open(path)
        .map_err(|error| {
            AppError::IoError(format!("cannot open image {}: {error}", path.display()))
        })?
        .with_guessed_format()
        .map_err(|error| AppError::ImageError(format!("cannot inspect image format: {error}")))?;
    if reader.format().is_none() {
        return Err(AppError::ImageError(format!(
            "unsupported image format: {}",
            path.display()
        )));
    }
    let (width, height) = reader.into_dimensions().map_err(classify_decode_error)?;
    let pixels = checked_pixel_count(width, height)?;
    Ok((width, height, pixels))
}

pub fn run_probe(fixture_dir: &Path) -> ImageFingerprintProbeResult {
    let mut diagnostics = Vec::new();
    let mut fingerprints = Vec::new();
    let entries = match std::fs::read_dir(fixture_dir) {
        Ok(entries) => entries,
        Err(error) => {
            return ImageFingerprintProbeResult {
                fingerprints,
                diagnostics: vec![format!("Cannot read fixture directory: {error}")],
                success: false,
            };
        }
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "jpg" | "jpeg" | "png" | "webp") {
            continue;
        }
        match fingerprint_image(&path) {
            Ok(fingerprint) => fingerprints.push(ImageFingerprintProbeEntry {
                fingerprint_version: FINGERPRINT_VERSION,
                file_path: fingerprint.file_path,
                format: fingerprint.format,
                width: fingerprint.width,
                height: fingerprint.height,
                file_size: fingerprint.file_size,
                blake3_bytes: fingerprint.blake3.len(),
                pixel_hash_bytes: fingerprint.pixel_hash.len(),
                block_hash_bits: fingerprint.block_hash_16.len() * 8,
                double_gradient_hash_bits: fingerprint.double_gradient_hash_32.len() * 8,
            }),
            Err(error) => {
                diagnostics.push(format!("Failed to fingerprint {}: {error}", path.display()))
            }
        }
    }
    let success = !fingerprints.is_empty();
    diagnostics.push(format!(
        "Fingerprint V2 probe processed {} image(s)",
        fingerprints.len()
    ));
    ImageFingerprintProbeResult {
        fingerprints,
        diagnostics,
        success,
    }
}

pub fn generate_test_samples(dir: &Path) -> Result<Vec<String>, AppError> {
    let mut image = image::RgbImage::new(64, 64);
    for y in 0..64 {
        for x in 0..64 {
            image.put_pixel(
                x,
                y,
                image::Rgb([
                    ((x * 4) % 256) as u8,
                    ((y * 4) % 256) as u8,
                    (((x + y) * 2) % 256) as u8,
                ]),
            );
        }
    }
    let mut created = Vec::new();
    for (name, format) in [
        ("test-sample.png", ImageFormat::Png),
        ("test-sample.jpg", ImageFormat::Jpeg),
        ("test-sample.webp", ImageFormat::WebP),
    ] {
        let path = dir.join(name);
        image.save_with_format(&path, format)?;
        created.push(path.display().to_string());
    }
    Ok(created)
}

fn fingerprint_bytes(path: &Path, file_bytes: &[u8]) -> Result<ImageFingerprintV2, AppError> {
    let file_size = file_bytes.len() as u64;
    let blake3 = blake3::hash(file_bytes).as_bytes().to_vec();
    let guessed_format = image::guess_format(file_bytes).map_err(|error| {
        AppError::ImageError(format!(
            "unsupported image format for {}: {error}",
            path.display()
        ))
    })?;
    let format = format_name(guessed_format);

    let dimension_reader = ImageReader::with_format(Cursor::new(file_bytes), guessed_format);
    let (source_width, source_height) = dimension_reader
        .into_dimensions()
        .map_err(classify_decode_error)?;
    let pixel_count = checked_pixel_count(source_width, source_height)?;
    if pixel_count > MAX_DECODED_IMAGE_PIXELS {
        return Err(AppError::ImageError(format!(
            "image pixel count exceeds product limit: {source_width}x{source_height} = {pixel_count} pixels (limit {MAX_DECODED_IMAGE_PIXELS})"
        )));
    }

    // The source is decoded exactly once. EXIF is parsed from the already-read
    // byte buffer, so orientation handling never performs a second file read.
    let mut decode_reader = ImageReader::with_format(Cursor::new(file_bytes), guessed_format);
    decode_reader.limits(Limits::no_limits());
    let decoded = decode_reader.decode().map_err(classify_decode_error)?;
    let orientation = read_exif_orientation(file_bytes, path);
    let oriented_rgba = apply_orientation(decoded.into_rgba8(), orientation);
    let (width, height) = oriented_rgba.dimensions();
    let pixel_hash = compute_pixel_hash(&oriented_rgba);

    let gray = DynamicImage::ImageRgba8(oriented_rgba).to_luma8();
    let block_thumbnail = image::imageops::resize(
        &gray,
        BLOCK_HASH_SIZE,
        BLOCK_HASH_SIZE,
        ImageFilterType::Triangle,
    );
    let fine_thumbnail_32 = image::imageops::resize(
        &gray,
        DOUBLE_GRADIENT_HASH_SIZE,
        DOUBLE_GRADIENT_HASH_SIZE,
        ImageFilterType::Triangle,
    );
    drop(gray);

    let block_hash_16 = compute_block_hash(&block_thumbnail);
    let double_gradient_hash_32 = compute_double_gradient_hash(&fine_thumbnail_32);
    let perceptual_eligible = evaluate_perceptual_eligibility(
        &fine_thumbnail_32,
        &block_hash_16,
        &double_gradient_hash_32,
    );
    let block_variants = TransformType::ALL
        .iter()
        .map(|&transform| {
            let transformed = transform_gray(&block_thumbnail, transform);
            BlockHashVariant {
                transform,
                hash: compute_block_hash(&transformed),
            }
        })
        .collect();

    Ok(ImageFingerprintV2 {
        file_path: path.display().to_string(),
        format,
        width,
        height,
        file_size,
        blake3,
        pixel_hash,
        block_hash_16,
        double_gradient_hash_32,
        perceptual_eligible,
        block_variants,
        fine_thumbnail_32,
    })
}

fn evaluate_perceptual_eligibility(
    thumbnail: &GrayImage,
    block_hash: &[u8],
    double_gradient_hash: &[u8],
) -> bool {
    let sample_count = (thumbnail.width() * thumbnail.height()) as usize;
    if sample_count == 0 {
        return false;
    }

    let mut sum = 0.0;
    let mut histogram = [0_usize; 16];
    for pixel in thumbnail.pixels() {
        let value = pixel[0] as usize;
        sum += value as f64;
        histogram[value / 16] += 1;
    }
    let mean = sum / sample_count as f64;
    let variance = thumbnail
        .pixels()
        .map(|pixel| {
            let delta = pixel[0] as f64 - mean;
            delta * delta
        })
        .sum::<f64>()
        / sample_count as f64;
    let standard_deviation = variance.sqrt();

    // Ignore isolated compression speckles when counting meaningful levels.
    let minimum_bin_population = (sample_count / 200).max(2);
    let effective_bins = histogram
        .iter()
        .filter(|&&count| count >= minimum_bin_population)
        .count();

    let mut edge_delta_sum = 0_u64;
    let mut edge_count = 0_u64;
    for y in 0..thumbnail.height() {
        for x in 0..thumbnail.width() {
            let value = thumbnail.get_pixel(x, y)[0];
            if x + 1 < thumbnail.width() {
                edge_delta_sum += value.abs_diff(thumbnail.get_pixel(x + 1, y)[0]) as u64;
                edge_count += 1;
            }
            if y + 1 < thumbnail.height() {
                edge_delta_sum += value.abs_diff(thumbnail.get_pixel(x, y + 1)[0]) as u64;
                edge_count += 1;
            }
        }
    }
    let mean_edge_delta = if edge_count == 0 {
        0.0
    } else {
        edge_delta_sum as f64 / edge_count as f64
    };

    let block_information = hash_information_bits(block_hash);
    let fine_information = hash_information_bits(double_gradient_hash);
    standard_deviation >= MIN_GRAYSCALE_STDDEV
        && effective_bins >= MIN_EFFECTIVE_GRAYSCALE_BINS
        && mean_edge_delta >= MIN_MEAN_EDGE_DELTA
        && block_information >= ((block_hash.len() * 8) as u32 / 64).max(1)
        && fine_information >= ((double_gradient_hash.len() * 8) as u32 / 64).max(1)
}

fn hash_information_bits(hash: &[u8]) -> u32 {
    let one_bits: u32 = hash.iter().map(|byte| byte.count_ones()).sum();
    let total_bits = (hash.len() * 8) as u32;
    one_bits.min(total_bits.saturating_sub(one_bits))
}

pub fn compute_double_gradient_for_transform(
    fine_thumbnail_32: &GrayImage,
    transform: TransformType,
) -> Vec<u8> {
    let transformed = transform_gray(fine_thumbnail_32, transform);
    compute_double_gradient_hash(&transformed)
}

fn compute_block_hash(thumbnail: &GrayImage) -> Vec<u8> {
    let hasher = HasherConfig::new()
        .hash_alg(HashAlg::Blockhash)
        .hash_size(BLOCK_HASH_SIZE, BLOCK_HASH_SIZE)
        .resize_filter(HashFilterType::Triangle)
        .to_hasher();
    hasher
        .hash_image(&DynamicImage::ImageLuma8(thumbnail.clone()))
        .as_bytes()
        .to_vec()
}

fn compute_double_gradient_hash(thumbnail: &GrayImage) -> Vec<u8> {
    let hasher = HasherConfig::new()
        .hash_alg(HashAlg::DoubleGradient)
        .hash_size(DOUBLE_GRADIENT_HASH_SIZE, DOUBLE_GRADIENT_HASH_SIZE)
        .resize_filter(HashFilterType::Triangle)
        .to_hasher();
    hasher
        .hash_image(&DynamicImage::ImageLuma8(thumbnail.clone()))
        .as_bytes()
        .to_vec()
}

fn compute_pixel_hash(rgba: &RgbaImage) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&rgba.width().to_le_bytes());
    hasher.update(&rgba.height().to_le_bytes());
    for pixel in rgba.pixels() {
        let [r, g, b, a] = pixel.0;
        if a == 0 {
            hasher.update(&[0, 0, 0, a]);
        } else {
            hasher.update(&[r, g, b, a]);
        }
    }
    hasher.finalize().as_bytes().to_vec()
}

fn checked_pixel_count(width: u32, height: u32) -> Result<u64, AppError> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| {
            AppError::ImageError(format!(
                "image dimension multiplication overflow: {width}x{height}"
            ))
        })
}

fn classify_decode_error(error: image::ImageError) -> AppError {
    match error {
        image::ImageError::Unsupported(error) => {
            AppError::ImageError(format!("unsupported image format: {error}"))
        }
        image::ImageError::Limits(error) => {
            let message = error.to_string();
            if message.to_ascii_lowercase().contains("memory")
                || message.to_ascii_lowercase().contains("allocation")
            {
                AppError::ImageError(format!("image memory allocation failed: {message}"))
            } else {
                AppError::ImageError(format!("image decode limit error: {message}"))
            }
        }
        image::ImageError::Decoding(error) => {
            AppError::ImageError(format!("corrupt or undecodable image: {error}"))
        }
        image::ImageError::IoError(error) => {
            AppError::IoError(format!("image read failed: {error}"))
        }
        other => AppError::ImageError(format!("image decode failed: {other}")),
    }
}

fn format_name(format: ImageFormat) -> String {
    match format {
        ImageFormat::Jpeg => "JPEG".to_string(),
        ImageFormat::Png => "PNG".to_string(),
        ImageFormat::WebP => "WebP".to_string(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
fn detect_format(bytes: &[u8]) -> String {
    image::guess_format(bytes)
        .map(format_name)
        .unwrap_or_else(|_| "unknown".to_string())
}

fn read_exif_orientation(bytes: &[u8], path: &Path) -> u32 {
    let mut cursor = Cursor::new(bytes);
    match Reader::new().read_from_container(&mut cursor) {
        Ok(exif) => match exif
            .get_field(Tag::Orientation, In::PRIMARY)
            .and_then(|field| field.value.get_uint(0))
        {
            Some(value @ 1..=8) => value,
            Some(value) => {
                tracing::debug!(source_path = %path.display(), orientation = value, "invalid EXIF orientation; using identity");
                1
            }
            None => {
                tracing::debug!(source_path = %path.display(), "EXIF orientation missing; using identity");
                1
            }
        },
        Err(error) => {
            tracing::debug!(source_path = %path.display(), error = %error, "EXIF metadata unavailable; using identity");
            1
        }
    }
}

fn apply_orientation(source: RgbaImage, orientation: u32) -> RgbaImage {
    let transform = match orientation {
        2 => TransformType::FlipH,
        3 => TransformType::Rot180,
        4 => TransformType::FlipV,
        5 => TransformType::Transpose,
        6 => TransformType::Rot90,
        7 => TransformType::Transverse,
        8 => TransformType::Rot270,
        _ => TransformType::Identity,
    };
    if transform == TransformType::Identity {
        source
    } else {
        transform_rgba(&source, transform)
    }
}

fn transform_gray(source: &GrayImage, transform: TransformType) -> GrayImage {
    let (out_width, out_height) =
        transformed_dimensions(source.width(), source.height(), transform);
    let mut output = GrayImage::new(out_width, out_height);
    for y in 0..out_height {
        for x in 0..out_width {
            let (source_x, source_y) =
                source_coordinates(source.width(), source.height(), x, y, transform);
            output.put_pixel(x, y, *source.get_pixel(source_x, source_y));
        }
    }
    output
}

fn transform_rgba(source: &RgbaImage, transform: TransformType) -> RgbaImage {
    let (out_width, out_height) =
        transformed_dimensions(source.width(), source.height(), transform);
    let mut output = RgbaImage::new(out_width, out_height);
    for y in 0..out_height {
        for x in 0..out_width {
            let (source_x, source_y) =
                source_coordinates(source.width(), source.height(), x, y, transform);
            output.put_pixel(x, y, *source.get_pixel(source_x, source_y));
        }
    }
    output
}

fn transformed_dimensions(width: u32, height: u32, transform: TransformType) -> (u32, u32) {
    match transform {
        TransformType::Rot90
        | TransformType::Rot270
        | TransformType::Transpose
        | TransformType::Transverse => (height, width),
        _ => (width, height),
    }
}

fn source_coordinates(
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    transform: TransformType,
) -> (u32, u32) {
    match transform {
        TransformType::Identity => (x, y),
        TransformType::Rot90 => (y, height - 1 - x),
        TransformType::Rot180 => (width - 1 - x, height - 1 - y),
        TransformType::Rot270 => (width - 1 - y, x),
        TransformType::FlipH => (width - 1 - x, y),
        TransformType::FlipV => (x, height - 1 - y),
        TransformType::Transpose => (y, x),
        TransformType::Transverse => (width - 1 - y, height - 1 - x),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ImageBuffer, Rgb};
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    fn patterned_rgba(width: u32, height: u32) -> RgbaImage {
        ImageBuffer::from_fn(width, height, |x, y| {
            image::Rgba([
                ((x * 31 + y * 7) % 251) as u8,
                ((x * 11 + y * 29) % 253) as u8,
                ((x * 17 + y * 19) % 247) as u8,
                255,
            ])
        })
    }

    fn write_png(path: &Path, image: &RgbaImage) {
        image.save_with_format(path, ImageFormat::Png).unwrap();
    }

    fn jpeg_with_orientation(image: &RgbaImage, orientation: u16) -> Vec<u8> {
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&DynamicImage::ImageRgba8(image.clone()))
            .unwrap();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);

        let mut exif = Vec::new();
        exif.extend_from_slice(b"Exif\0\0");
        exif.extend_from_slice(b"II");
        exif.extend_from_slice(&42u16.to_le_bytes());
        exif.extend_from_slice(&8u32.to_le_bytes());
        exif.extend_from_slice(&1u16.to_le_bytes());
        exif.extend_from_slice(&0x0112u16.to_le_bytes());
        exif.extend_from_slice(&3u16.to_le_bytes());
        exif.extend_from_slice(&1u32.to_le_bytes());
        exif.extend_from_slice(&orientation.to_le_bytes());
        exif.extend_from_slice(&0u16.to_le_bytes());
        exif.extend_from_slice(&0u32.to_le_bytes());

        let segment_len = (exif.len() + 2) as u16;
        let mut output = Vec::with_capacity(jpeg.len() + exif.len() + 4);
        output.extend_from_slice(&jpeg[..2]);
        output.extend_from_slice(&[0xff, 0xe1]);
        output.extend_from_slice(&segment_len.to_be_bytes());
        output.extend_from_slice(&exif);
        output.extend_from_slice(&jpeg[2..]);
        output
    }

    #[test]
    fn full_file_hash_is_deterministic_and_complete() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("same.png");
        write_png(&path, &patterned_rgba(48, 32));
        let first = fingerprint_image(&path).unwrap();
        let second = fingerprint_image(&path).unwrap();
        assert_eq!(first.blake3, second.blake3);
        assert_eq!(first.blake3.len(), 32);
    }

    #[test]
    fn fingerprint_v2_golden_hashes_are_unchanged() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("golden.png");
        write_png(&path, &patterned_rgba(37, 23));
        let fingerprint = fingerprint_image(&path).unwrap();
        let hex = |bytes: &[u8]| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        assert_eq!(
            hex(&fingerprint.pixel_hash),
            "5a850f440d66c44be7b4151ad9ceb0d54c1a2eb3d964dfc4f1512724119802be"
        );
        assert_eq!(
            hex(&fingerprint.block_hash_16),
            "e083f0e07cf01e7e073fc30fe083f0f07cf01e78071f830fe083f0c07ae81f78"
        );
        assert_eq!(
            hex(&fingerprint.double_gradient_hash_32),
            "77b937b817bb873ba39bb1c3bad1fbd81bdc8b5d831dd1cfd8e1dce07dec0d6ecd2e9fcf8fe7cff3e7f3e7f9f378f13c793e7c9e3c9f1ecf9fc7cfe7cff3e7f1f379f33c"
        );
        assert_eq!(fingerprint.width, 37);
        assert_eq!(fingerprint.height, 23);
    }

    #[test]
    fn identity_orientation_reuses_the_owned_pixel_buffer() {
        let source = patterned_rgba(37, 23);
        let original_ptr = source.as_raw().as_ptr();
        let oriented = apply_orientation(source, 1);
        assert_eq!(original_ptr, oriented.as_raw().as_ptr());
    }

    #[test]
    fn product_pixel_limit_rejects_before_full_decode() {
        let width = 30_000u32;
        let height = 30_000u32;
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let writer = encoder.write_header().unwrap();
            writer.finish().unwrap();
        }
        // `into_dimensions` requires the structural presence of IDAT but does
        // not inflate it. Insert an empty IDAT before IEND so the test proves
        // the product limit is evaluated from the header before full decode.
        let iend = bytes.split_off(bytes.len() - 12);
        bytes.extend_from_slice(&[0, 0, 0, 0, b'I', b'D', b'A', b'T', 0x35, 0xaf, 0x06, 0x1e]);
        bytes.extend_from_slice(&iend);
        let error = fingerprint_bytes(Path::new("over-limit.png"), &bytes).unwrap_err();
        assert!(error.to_string().contains("product limit"), "{error}");
    }

    #[test]
    #[ignore = "large-memory integration: decodes and fingerprints a 15001x15001 PNG"]
    fn fingerprints_15001_square_png_without_decoder_limit_failure() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("15001-square.png");
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = png::Encoder::new(file, 15_001, 15_001);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        {
            let mut stream = writer.stream_writer_with_size(1024 * 1024).unwrap();
            let row = vec![0x80; 15_001];
            for y in 0..15_001u32 {
                let mut varied = row.clone();
                let index = (y as usize * 97) % varied.len();
                varied[index] = (y % 251) as u8;
                stream.write_all(&varied).unwrap();
            }
            stream.finish().unwrap();
        }
        writer.finish().unwrap();

        let fingerprint = fingerprint_image(&path).unwrap();
        assert_eq!((fingerprint.width, fingerprint.height), (15_001, 15_001));
        assert_eq!(fingerprint.pixel_hash.len(), 32);
        assert_eq!(
            fingerprint.block_hash_16.len() * 8,
            BLOCK_HASH_BIT_LENGTH as usize
        );
        assert_eq!(
            fingerprint.double_gradient_hash_32.len() * 8,
            DOUBLE_GRADIENT_HASH_BIT_LENGTH as usize
        );
    }

    #[test]
    fn equal_pixels_in_different_encodings_have_equal_pixel_hashes() {
        let tmp = TempDir::new().unwrap();
        let image = patterned_rgba(24, 18);
        let png = tmp.path().join("same-pixels.png");
        let bmp = tmp.path().join("same-pixels.bmp");
        image.save_with_format(&png, ImageFormat::Png).unwrap();
        image.save_with_format(&bmp, ImageFormat::Bmp).unwrap();

        let first = fingerprint_image(&png).unwrap();
        let second = fingerprint_image(&bmp).unwrap();
        assert_ne!(first.blake3, second.blake3);
        assert_eq!(first.pixel_hash, second.pixel_hash);
        assert_eq!(first.pixel_hash.len(), 32);
    }

    #[test]
    fn different_pixels_have_different_pixel_hashes() {
        let mut first = patterned_rgba(8, 8);
        let mut second = first.clone();
        second.put_pixel(3, 4, image::Rgba([1, 2, 3, 255]));
        assert_ne!(compute_pixel_hash(&first), compute_pixel_hash(&second));
        first.put_pixel(3, 4, image::Rgba([1, 2, 3, 255]));
        assert_eq!(compute_pixel_hash(&first), compute_pixel_hash(&second));
    }

    #[test]
    fn transparent_hidden_rgb_does_not_change_pixel_hash_but_alpha_is_preserved() {
        let mut first = RgbaImage::new(2, 1);
        first.put_pixel(0, 0, image::Rgba([1, 2, 3, 0]));
        first.put_pixel(1, 0, image::Rgba([4, 5, 6, 127]));
        let mut second = first.clone();
        second.put_pixel(0, 0, image::Rgba([250, 249, 248, 0]));
        assert_eq!(compute_pixel_hash(&first), compute_pixel_hash(&second));
        second.put_pixel(1, 0, image::Rgba([4, 5, 6, 126]));
        assert_ne!(compute_pixel_hash(&first), compute_pixel_hash(&second));
    }

    #[test]
    fn exif_rotation_matches_physically_rotated_pixels() {
        let tmp = TempDir::new().unwrap();
        let source = patterned_rgba(17, 11);
        let exif_path = tmp.path().join("oriented.jpg");
        let exif_bytes = jpeg_with_orientation(&source, 6);
        fs::write(&exif_path, &exif_bytes).unwrap();

        let decoded = image::load_from_memory(&exif_bytes).unwrap().to_rgba8();
        let physical = apply_orientation(decoded, 6);
        let physical_path = tmp.path().join("physical.png");
        write_png(&physical_path, &physical);

        let exif_fingerprint = fingerprint_image(&exif_path).unwrap();
        let physical_fingerprint = fingerprint_image(&physical_path).unwrap();
        assert_eq!(exif_fingerprint.pixel_hash, physical_fingerprint.pixel_hash);
        assert_eq!((exif_fingerprint.width, exif_fingerprint.height), (11, 17));
    }

    #[test]
    fn all_exif_orientations_match_their_geometry_transform() {
        let source = patterned_rgba(5, 3);
        for orientation in 1..=8 {
            let transformed = apply_orientation(source.clone(), orientation);
            let transform = match orientation {
                2 => TransformType::FlipH,
                3 => TransformType::Rot180,
                4 => TransformType::FlipV,
                5 => TransformType::Transpose,
                6 => TransformType::Rot90,
                7 => TransformType::Transverse,
                8 => TransformType::Rot270,
                _ => TransformType::Identity,
            };
            assert_eq!(transformed, transform_rgba(&source, transform));
        }
    }

    #[test]
    fn configured_hash_lengths_match_v2_policy() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("hashes.png");
        write_png(&path, &patterned_rgba(96, 64));
        let fingerprint = fingerprint_image(&path).unwrap();
        assert_eq!(fingerprint.block_hash_16.len() * 8, 16 * 16);
        assert_eq!(
            fingerprint.double_gradient_hash_32.len() * 8,
            DOUBLE_GRADIENT_HASH_BIT_LENGTH as usize
        );
        assert_eq!(fingerprint.block_variants.len(), 8);
        assert!(fingerprint
            .block_variants
            .iter()
            .any(|variant| variant.transform == TransformType::Identity));
        assert!(fingerprint.perceptual_eligible);
    }

    #[test]
    fn flat_and_near_blank_images_are_not_perceptually_eligible() {
        let tmp = TempDir::new().unwrap();
        let fixtures = [
            ("white.png", image::Rgba([255, 255, 255, 255])),
            ("black.png", image::Rgba([0, 0, 0, 255])),
            ("red.png", image::Rgba([255, 0, 0, 255])),
            ("blue.png", image::Rgba([0, 0, 255, 255])),
            ("gray-80.png", image::Rgba([80, 80, 80, 255])),
            ("gray-160.png", image::Rgba([160, 160, 160, 255])),
        ];
        let mut fingerprints = Vec::new();
        for (name, color) in fixtures {
            let path = tmp.path().join(name);
            write_png(&path, &RgbaImage::from_pixel(128, 128, color));
            let fingerprint = fingerprint_image(&path).unwrap();
            assert!(!fingerprint.perceptual_eligible, "fixture {name}");
            fingerprints.push(fingerprint);
        }
        assert_ne!(fingerprints[0].pixel_hash, fingerprints[1].pixel_hash);
        assert_ne!(fingerprints[2].pixel_hash, fingerprints[3].pixel_hash);
        assert_ne!(fingerprints[4].pixel_hash, fingerprints[5].pixel_hash);

        let blank_path = tmp.path().join("near-blank.png");
        let content_path = tmp.path().join("near-blank-with-content.png");
        let blank = RgbaImage::from_pixel(128, 128, image::Rgba([250, 250, 250, 255]));
        let mut tiny_content = blank.clone();
        for y in 61..67 {
            for x in 59..65 {
                tiny_content.put_pixel(x, y, image::Rgba([40, 40, 40, 255]));
            }
        }
        write_png(&blank_path, &blank);
        write_png(&content_path, &tiny_content);
        let blank_fingerprint = fingerprint_image(&blank_path).unwrap();
        let content_fingerprint = fingerprint_image(&content_path).unwrap();
        assert_ne!(blank_fingerprint.pixel_hash, content_fingerprint.pixel_hash);
        assert!(!blank_fingerprint.perceptual_eligible);
        assert!(!content_fingerprint.perceptual_eligible);
    }

    #[test]
    fn hamming_distance_is_normalized_by_actual_length() {
        let identical = hamming_distance(&[0, 0], &[0, 0]).unwrap();
        assert_eq!(identical.raw_distance, 0);
        assert_eq!(identical.normalized_distance, 0.0);
        assert_eq!(identical.bit_length, 16);

        let opposite = hamming_distance(&[0, 0], &[0xff, 0xff]).unwrap();
        assert_eq!(opposite.raw_distance, 16);
        assert_eq!(opposite.normalized_distance, 1.0);
        assert!((0.0..=1.0).contains(&opposite.normalized_distance));
    }

    #[test]
    fn hamming_distance_rejects_different_lengths() {
        let error = hamming_distance(&[0], &[0, 0]).unwrap_err();
        assert!(error.to_string().contains("different lengths"));
    }

    #[test]
    fn transforms_recall_equivalent_block_hashes() {
        let source = DynamicImage::ImageRgba8(patterned_rgba(64, 64)).to_luma8();
        let small = image::imageops::resize(
            &source,
            BLOCK_HASH_SIZE,
            BLOCK_HASH_SIZE,
            ImageFilterType::Triangle,
        );
        let identity = compute_block_hash(&small);
        for transform in [
            TransformType::Identity,
            TransformType::Rot90,
            TransformType::Rot180,
            TransformType::Rot270,
            TransformType::FlipH,
            TransformType::FlipV,
        ] {
            let transformed = transform_gray(&small, transform);
            let transformed_hash = compute_block_hash(&transformed);
            let inverse_variant = TransformType::ALL
                .iter()
                .map(|&candidate| {
                    let candidate_hash =
                        compute_block_hash(&transform_gray(&transformed, candidate));
                    let distance = hamming_distance(&candidate_hash, &identity).unwrap();
                    (candidate, distance.raw_distance)
                })
                .min_by_key(|(_, distance)| *distance)
                .unwrap();
            assert_eq!(inverse_variant.1, 0, "transform {transform}");
            assert_eq!(transformed_hash.len(), identity.len());
        }
    }

    #[test]
    fn worker_count_and_recall_radius_follow_fixed_policy() {
        assert!((2..=8).contains(&fingerprint_worker_count()));
        assert_eq!(recall_radius(BLOCK_HASH_BIT_LENGTH), 31);
        assert_eq!(MAX_RECALL_CANDIDATES_PER_IMAGE, 256);
    }

    #[test]
    fn similarity_uses_documented_weights() {
        let similarity = weighted_similarity(0.04, 0.08);
        assert!((similarity - 0.936).abs() < f64::EPSILON);
    }

    #[test]
    fn format_probe_handles_expected_types() {
        let rgb = ImageBuffer::from_fn(4, 4, |x, y| Rgb([(x * 20) as u8, (y * 30) as u8, 10]));
        let mut jpeg = Vec::new();
        JpegEncoder::new(&mut jpeg)
            .encode_image(&DynamicImage::ImageRgb8(rgb))
            .unwrap();
        assert_eq!(detect_format(&jpeg), "JPEG");
    }
}

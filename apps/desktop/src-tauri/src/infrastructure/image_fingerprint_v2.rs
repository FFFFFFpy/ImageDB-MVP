use crate::domain::import_state::TransformType;
use crate::error::AppError;
use exif::{In, Reader, Tag};
use image::imageops::FilterType as ImageFilterType;
use image::{DynamicImage, GrayImage, ImageFormat, RgbaImage};
use image_hasher::{FilterType as HashFilterType, HashAlg, HasherConfig};
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
    pub block_variants: Vec<BlockHashVariant>,
    /// Small, post-orientation grayscale image retained only while the scan
    /// is active so the winning transform can be fine-hashed once.
    pub fine_thumbnail_32: GrayImage,
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

fn fingerprint_bytes(path: &Path, file_bytes: &[u8]) -> Result<ImageFingerprintV2, AppError> {
    let file_size = file_bytes.len() as u64;
    let blake3 = blake3::hash(file_bytes).as_bytes().to_vec();
    let format = detect_format(file_bytes);

    // The source is decoded exactly once. EXIF is parsed from the already-read
    // byte buffer, so orientation handling never performs a second file read.
    let decoded = image::load_from_memory(file_bytes)?;
    let orientation = read_exif_orientation(file_bytes, path);
    let oriented_rgba = apply_orientation(decoded.to_rgba8(), orientation);
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
        block_variants,
        fine_thumbnail_32,
    })
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

fn detect_format(bytes: &[u8]) -> String {
    match image::guess_format(bytes) {
        Ok(ImageFormat::Jpeg) => "JPEG".to_string(),
        Ok(ImageFormat::Png) => "PNG".to_string(),
        Ok(ImageFormat::WebP) => "WebP".to_string(),
        Ok(format) => format!("{format:?}"),
        Err(_) => "unknown".to_string(),
    }
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
    transform_rgba(&source, transform)
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
    fn equal_pixels_in_different_encodings_have_equal_pixel_hashes() {
        let tmp = TempDir::new().unwrap();
        let image = patterned_rgba(24, 18);
        let png_a = tmp.path().join("a.png");
        let png_b = tmp.path().join("b.png");
        write_png(&png_a, &image);
        write_png(&png_b, &image);
        let mut bytes = fs::read(&png_b).unwrap();
        bytes.extend_from_slice(b"ignored trailing metadata bytes");
        fs::write(&png_b, bytes).unwrap();

        let first = fingerprint_image(&png_a).unwrap();
        let second = fingerprint_image(&png_b).unwrap();
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

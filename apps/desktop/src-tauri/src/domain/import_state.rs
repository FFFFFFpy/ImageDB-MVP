use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImportRunState {
    Scanning,
    Fingerprinting,
    DetectingDuplicates,
    Completed,
    Cancelled,
    Failed,
}

impl fmt::Display for ImportRunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scanning => write!(f, "scanning"),
            Self::Fingerprinting => write!(f, "fingerprinting"),
            Self::DetectingDuplicates => write!(f, "detecting_duplicates"),
            Self::Completed => write!(f, "completed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl ImportRunState {
    #[allow(dead_code)]
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "scanning" => Some(Self::Scanning),
            "fingerprinting" => Some(Self::Fingerprinting),
            "detecting_duplicates" => Some(Self::DetectingDuplicates),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImportAlbumState {
    Pending,
    Scanning,
    Fingerprinting,
    Completed,
    Failed,
}

impl fmt::Display for ImportAlbumState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Scanning => write!(f, "scanning"),
            Self::Fingerprinting => write!(f, "fingerprinting"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl ImportAlbumState {
    #[allow(dead_code)]
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "scanning" => Some(Self::Scanning),
            "fingerprinting" => Some(Self::Fingerprinting),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImportImageState {
    Pending,
    Fingerprinted,
    Failed,
}

impl fmt::Display for ImportImageState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Fingerprinted => write!(f, "fingerprinted"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl ImportImageState {
    #[allow(dead_code)]
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "fingerprinted" => Some(Self::Fingerprinted),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DecodeState {
    Pending,
    Decoded,
    Failed,
}

impl fmt::Display for DecodeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Decoded => write!(f, "decoded"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DuplicateScope {
    IntraAlbum,
    Library,
}

impl fmt::Display for DuplicateScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IntraAlbum => write!(f, "intra_album"),
            Self::Library => write!(f, "library"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MatchType {
    FileExact,
    PixelExact,
    PerceptualNear,
    PerceptualSimilar,
}

impl fmt::Display for MatchType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileExact => write!(f, "file_exact"),
            Self::PixelExact => write!(f, "pixel_exact"),
            Self::PerceptualNear => write!(f, "perceptual_near"),
            Self::PerceptualSimilar => write!(f, "perceptual_similar"),
        }
    }
}

impl MatchType {
    #[allow(dead_code)]
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "file_exact" => Some(Self::FileExact),
            "pixel_exact" => Some(Self::PixelExact),
            "perceptual_near" => Some(Self::PerceptualNear),
            "perceptual_similar" => Some(Self::PerceptualSimilar),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TransformType {
    Identity,
    Rot90,
    Rot180,
    Rot270,
    FlipH,
    FlipV,
    Transpose,
    Transverse,
}

impl TransformType {
    pub const ALL: [Self; 8] = [
        Self::Identity,
        Self::Rot90,
        Self::Rot180,
        Self::Rot270,
        Self::FlipH,
        Self::FlipV,
        Self::Transpose,
        Self::Transverse,
    ];
}

impl fmt::Display for TransformType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Identity => "identity",
            Self::Rot90 => "rot90",
            Self::Rot180 => "rot180",
            Self::Rot270 => "rot270",
            Self::FlipH => "flip_h",
            Self::FlipV => "flip_v",
            Self::Transpose => "transpose",
            Self::Transverse => "transverse",
        };
        write!(f, "{s}")
    }
}

impl TransformType {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "identity" => Some(Self::Identity),
            "rot90" => Some(Self::Rot90),
            "rot180" => Some(Self::Rot180),
            "rot270" => Some(Self::Rot270),
            "flip_h" => Some(Self::FlipH),
            "flip_v" => Some(Self::FlipV),
            "transpose" => Some(Self::Transpose),
            "transverse" => Some(Self::Transverse),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Decision {
    AutoDuplicate,
    Review,
}

impl fmt::Display for Decision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AutoDuplicate => write!(f, "auto_duplicate"),
            Self::Review => write!(f, "review"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DecisionSource {
    ExactRule,
    PerceptualRule,
}

impl fmt::Display for DecisionSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExactRule => write!(f, "exact_rule"),
            Self::PerceptualRule => write!(f, "perceptual_rule"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MatchingStrategy {
    Strict,
    Balanced,
    Loose,
}

impl fmt::Display for MatchingStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Strict => write!(f, "strict"),
            Self::Balanced => write!(f, "balanced"),
            Self::Loose => write!(f, "loose"),
        }
    }
}

impl MatchingStrategy {
    pub fn perceptual_thresholds(self) -> PerceptualThresholds {
        match self {
            Self::Strict => PerceptualThresholds {
                near_max_distance: 4,
                similar_max_total: 12,
                auto_decide: true,
            },
            Self::Balanced => PerceptualThresholds {
                near_max_distance: 8,
                similar_max_total: 24,
                auto_decide: true,
            },
            Self::Loose => PerceptualThresholds {
                near_max_distance: 12,
                similar_max_total: 40,
                auto_decide: false,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PerceptualThresholds {
    pub near_max_distance: i32,
    pub similar_max_total: i32,
    pub auto_decide: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProgress {
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

impl ScanProgress {
    pub fn idle() -> Self {
        Self {
            state: "idle".to_string(),
            import_run_id: None,
            current_stage: "idle".to_string(),
            current_album: None,
            processed_images: 0,
            total_albums: 0,
            total_images: 0,
            duplicate_count: 0,
            error_count: 0,
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanSourceInfo {
    pub path: String,
    pub albums: Vec<String>,
    pub album_count: u32,
}

pub const SUPPORTED_IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

pub const SCAN_POLICY_VERSION: &str = "2.0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_run_state_round_trip() {
        assert_eq!(
            ImportRunState::from_str_opt(&ImportRunState::Scanning.to_string()),
            Some(ImportRunState::Scanning)
        );
        assert_eq!(
            ImportRunState::from_str_opt(&ImportRunState::Completed.to_string()),
            Some(ImportRunState::Completed)
        );
        assert_eq!(ImportRunState::from_str_opt("unknown"), None);
    }

    #[test]
    fn import_album_state_round_trip() {
        assert_eq!(
            ImportAlbumState::from_str_opt(&ImportAlbumState::Fingerprinting.to_string()),
            Some(ImportAlbumState::Fingerprinting)
        );
        assert_eq!(ImportAlbumState::from_str_opt("unknown"), None);
    }

    #[test]
    fn import_image_state_round_trip() {
        assert_eq!(
            ImportImageState::from_str_opt(&ImportImageState::Fingerprinted.to_string()),
            Some(ImportImageState::Fingerprinted)
        );
        assert_eq!(ImportImageState::from_str_opt("unknown"), None);
    }

    #[test]
    fn match_type_round_trip() {
        for mt in [
            MatchType::FileExact,
            MatchType::PixelExact,
            MatchType::PerceptualNear,
            MatchType::PerceptualSimilar,
        ] {
            assert_eq!(MatchType::from_str_opt(&mt.to_string()), Some(mt));
        }
        assert_eq!(MatchType::from_str_opt("bogus"), None);
    }

    #[test]
    fn transform_type_round_trip() {
        for tt in TransformType::ALL {
            assert_eq!(TransformType::from_str_opt(&tt.to_string()), Some(tt));
        }
        assert_eq!(TransformType::from_str_opt("bogus"), None);
    }

    #[test]
    fn decision_display() {
        assert_eq!(Decision::AutoDuplicate.to_string(), "auto_duplicate");
        assert_eq!(Decision::Review.to_string(), "review");
    }

    #[test]
    fn decision_source_display() {
        assert_eq!(DecisionSource::ExactRule.to_string(), "exact_rule");
        assert_eq!(
            DecisionSource::PerceptualRule.to_string(),
            "perceptual_rule"
        );
    }

    #[test]
    fn matching_strategy_thresholds() {
        let strict = MatchingStrategy::Strict.perceptual_thresholds();
        let balanced = MatchingStrategy::Balanced.perceptual_thresholds();
        let loose = MatchingStrategy::Loose.perceptual_thresholds();

        assert!(strict.near_max_distance < balanced.near_max_distance);
        assert!(balanced.near_max_distance < loose.near_max_distance);
        assert!(strict.similar_max_total < balanced.similar_max_total);
        assert!(balanced.similar_max_total < loose.similar_max_total);
        assert!(strict.auto_decide);
        assert!(balanced.auto_decide);
        assert!(!loose.auto_decide);
    }
}

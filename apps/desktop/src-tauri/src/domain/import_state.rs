use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImportRunState {
    Created,
    Scanning,
    Fingerprinting,
    DetectingDuplicates,
    Analyzing,
    ReviewRequired,
    ReadyToCommit,
    Committing,
    RecoveryRequired,
    Completed,
    Cancelled,
    Failed,
    Abandoned,
}

impl fmt::Display for ImportRunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Scanning => write!(f, "scanning"),
            Self::Fingerprinting => write!(f, "fingerprinting"),
            Self::DetectingDuplicates => write!(f, "detecting_duplicates"),
            Self::Analyzing => write!(f, "analyzing"),
            Self::ReviewRequired => write!(f, "review_required"),
            Self::ReadyToCommit => write!(f, "ready_to_commit"),
            Self::Committing => write!(f, "committing"),
            Self::RecoveryRequired => write!(f, "recovery_required"),
            Self::Completed => write!(f, "completed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Failed => write!(f, "failed"),
            Self::Abandoned => write!(f, "abandoned"),
        }
    }
}

impl ImportRunState {
    #[allow(dead_code)]
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "created" => Some(Self::Created),
            "scanning" => Some(Self::Scanning),
            "fingerprinting" => Some(Self::Fingerprinting),
            "detecting_duplicates" => Some(Self::DetectingDuplicates),
            "analyzing" => Some(Self::Analyzing),
            "review_required" => Some(Self::ReviewRequired),
            "ready_to_commit" => Some(Self::ReadyToCommit),
            "committing" => Some(Self::Committing),
            "recovery_required" => Some(Self::RecoveryRequired),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImportAlbumState {
    Pending,
    Analyzing,
    Analyzed,
    ReviewRequired,
    Failed,
}

impl fmt::Display for ImportAlbumState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Analyzing => write!(f, "analyzing"),
            Self::Analyzed => write!(f, "analyzed"),
            Self::ReviewRequired => write!(f, "review_required"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl ImportAlbumState {
    #[allow(dead_code)]
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "analyzing" => Some(Self::Analyzing),
            "analyzed" => Some(Self::Analyzed),
            "review_required" => Some(Self::ReviewRequired),
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
    CrossAlbum,
    Library,
}

impl fmt::Display for DuplicateScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IntraAlbum => write!(f, "intra_album"),
            Self::CrossAlbum => write!(f, "cross_album"),
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
pub enum ReviewDecisionAction {
    KeepSource,
    KeepCandidate,
    KeepAll,
    SkipAlbum,
}

impl fmt::Display for ReviewDecisionAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeepSource => write!(f, "keep_source"),
            Self::KeepCandidate => write!(f, "keep_candidate"),
            Self::KeepAll => write!(f, "keep_all"),
            Self::SkipAlbum => write!(f, "skip_album"),
        }
    }
}

impl ReviewDecisionAction {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "keep_source" => Some(Self::KeepSource),
            "keep_candidate" => Some(Self::KeepCandidate),
            "keep_all" => Some(Self::KeepAll),
            "skip_album" => Some(Self::SkipAlbum),
            _ => None,
        }
    }
}

pub const REVIEW_DECISION_VALUES: &str = "keep_source, keep_candidate, keep_all, skip_album";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewCandidateSummary {
    pub candidate_id: String,
    pub source_image_id: String,
    pub candidate_source_image_id: Option<String>,
    pub candidate_library_image_id: Option<String>,
    pub scope: String,
    pub match_type: String,
    pub transform_type: Option<String>,
    pub confidence: Option<f64>,
    pub album_name: String,
    pub has_decision: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewCandidateDetail {
    pub candidate_id: String,
    pub source_image_id: String,
    pub source_image_path: String,
    pub source_image_file_size: i64,
    pub source_image_width: Option<i32>,
    pub source_image_height: Option<i32>,
    pub candidate_source_image_id: Option<String>,
    pub candidate_source_image_path: Option<String>,
    pub candidate_source_image_file_size: Option<i64>,
    pub candidate_source_image_width: Option<i32>,
    pub candidate_source_image_height: Option<i32>,
    pub candidate_library_image_id: Option<String>,
    pub candidate_library_image_path: Option<String>,
    pub candidate_library_image_file_size: Option<i64>,
    pub candidate_library_image_width: Option<i32>,
    pub candidate_library_image_height: Option<i32>,
    pub scope: String,
    pub match_type: String,
    pub blake3_equal: bool,
    pub pixel_hash_equal: bool,
    pub gradient_distance: Option<i32>,
    pub block_distance: Option<i32>,
    pub median_distance: Option<i32>,
    pub transform_type: Option<String>,
    pub confidence: Option<f64>,
    pub album_name: String,
    pub album_id: String,
    pub existing_decision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewProgress {
    pub import_run_id: String,
    pub total_review_candidates: u32,
    pub decided_count: u32,
    pub remaining_count: u32,
    pub all_decided: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPlanImage {
    pub image_id: String,
    pub source_path: String,
    pub relative_path: String,
    pub file_size: i64,
    pub album_name: String,
    pub album_id: String,
    pub source_album_id: String,
    pub included: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPlanAlbum {
    pub album_id: String,
    pub album_name: String,
    pub included: bool,
    pub image_count: u32,
    pub total_size: i64,
    pub images: Vec<ImportPlanImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPlan {
    pub import_run_id: String,
    pub plan_hash: Option<String>,
    pub total_albums: u32,
    pub total_images: u32,
    pub kept_images: Vec<ImportPlanImage>,
    pub excluded_count: u32,
    pub skipped_albums: Vec<String>,
    pub albums: Vec<ImportPlanAlbum>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitProgress {
    pub state: String,
    pub import_run_id: String,
    pub current_stage: String,
    pub current_album: Option<String>,
    pub albums_total: u32,
    pub albums_completed: u32,
    pub albums_skipped: u32,
    pub albums_failed: u32,
    pub images_committed: u32,
    pub errors: Vec<String>,
}

impl CommitProgress {
    pub fn idle(import_run_id: &str) -> Self {
        Self {
            state: "idle".to_string(),
            import_run_id: import_run_id.to_string(),
            current_stage: "idle".to_string(),
            current_album: None,
            albums_total: 0,
            albums_completed: 0,
            albums_skipped: 0,
            albums_failed: 0,
            images_committed: 0,
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitAlbumResult {
    pub album_name: String,
    pub status: String,
    pub images_committed: u32,
    pub target_path: Option<String>,
    pub manifest_path: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitResult {
    pub import_run_id: String,
    pub albums_total: u32,
    pub albums_committed: u32,
    pub albums_skipped: u32,
    pub albums_failed: u32,
    pub images_committed: u32,
    pub album_results: Vec<CommitAlbumResult>,
    pub errors: Vec<String>,
    pub state: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_run_state_round_trip() {
        for state in [
            ImportRunState::Created,
            ImportRunState::Scanning,
            ImportRunState::Fingerprinting,
            ImportRunState::DetectingDuplicates,
            ImportRunState::Analyzing,
            ImportRunState::ReviewRequired,
            ImportRunState::ReadyToCommit,
            ImportRunState::Committing,
            ImportRunState::RecoveryRequired,
            ImportRunState::Completed,
            ImportRunState::Cancelled,
            ImportRunState::Failed,
        ] {
            assert_eq!(
                ImportRunState::from_str_opt(&state.to_string()),
                Some(state)
            );
        }
        assert_eq!(ImportRunState::from_str_opt("unknown"), None);
    }

    #[test]
    fn import_album_state_round_trip() {
        for state in [
            ImportAlbumState::Pending,
            ImportAlbumState::Analyzing,
            ImportAlbumState::Analyzed,
            ImportAlbumState::ReviewRequired,
            ImportAlbumState::Failed,
        ] {
            assert_eq!(
                ImportAlbumState::from_str_opt(&state.to_string()),
                Some(state)
            );
        }
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
    fn review_decision_action_round_trip() {
        for action in [
            ReviewDecisionAction::KeepSource,
            ReviewDecisionAction::KeepCandidate,
            ReviewDecisionAction::KeepAll,
            ReviewDecisionAction::SkipAlbum,
        ] {
            assert_eq!(
                ReviewDecisionAction::from_str_opt(&action.to_string()),
                Some(action)
            );
        }
        assert_eq!(ReviewDecisionAction::from_str_opt("bogus"), None);
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

    #[test]
    fn commit_progress_idle_defaults() {
        let p = CommitProgress::idle("run-1");
        assert_eq!(p.state, "idle");
        assert_eq!(p.import_run_id, "run-1");
        assert_eq!(p.albums_total, 0);
        assert_eq!(p.albums_completed, 0);
        assert_eq!(p.albums_skipped, 0);
        assert_eq!(p.albums_failed, 0);
        assert_eq!(p.images_committed, 0);
        assert!(p.errors.is_empty());
        assert!(p.current_album.is_none());
    }
}

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
}

impl fmt::Display for MatchType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileExact => write!(f, "file_exact"),
            Self::PixelExact => write!(f, "pixel_exact"),
        }
    }
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

pub const SCAN_POLICY_VERSION: &str = "1.0";

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
}

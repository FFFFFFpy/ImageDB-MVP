use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityProbe {
    pub status: CapabilityStatus,
    pub detail: String,
}

impl CapabilityProbe {
    fn supported(detail: impl Into<String>) -> Self {
        Self {
            status: CapabilityStatus::Supported,
            detail: detail.into(),
        }
    }

    fn unsupported(detail: impl Into<String>) -> Self {
        Self {
            status: CapabilityStatus::Unsupported,
            detail: detail.into(),
        }
    }

    fn unknown(detail: impl Into<String>) -> Self {
        Self {
            status: CapabilityStatus::Unknown,
            detail: detail.into(),
        }
    }

    fn is_supported(&self) -> bool {
        self.status == CapabilityStatus::Supported
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageType {
    MountedShared,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishStrategy {
    StrongLocal,
    ConservativeMounted,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageCapabilities {
    pub root: String,
    pub probe_version: u32,
    pub probed_at: DateTime<Utc>,
    pub storage_type: StorageType,
    pub publish_strategy: PublishStrategy,
    pub strategy_reasons: Vec<String>,
    pub probe_dir_cleaned: bool,
    pub readable: CapabilityProbe,
    pub writable: CapabilityProbe,
    pub can_create_dir: CapabilityProbe,
    pub same_dir_file_rename: CapabilityProbe,
    pub same_root_rename: CapabilityProbe,
    pub directory_rename: CapabilityProbe,
    pub overwrite_rename: CapabilityProbe,
    pub file_sync_all: CapabilityProbe,
    pub parent_dir_sync: CapabilityProbe,
    pub case_sensitive: CapabilityProbe,
    pub unicode_normalization: CapabilityProbe,
    pub max_path: CapabilityProbe,
    pub max_component: CapabilityProbe,
    pub file_lock: CapabilityProbe,
    pub timestamp_precision: CapabilityProbe,
    pub free_space: CapabilityProbe,
    pub volume_identity: CapabilityProbe,
    pub diagnostics: Vec<String>,
}

pub fn probe_storage_capabilities(root: impl AsRef<Path>) -> StorageCapabilities {
    let root = root.as_ref().to_path_buf();
    let root_display = root.display().to_string();
    let storage_type = detect_storage_type(&root);
    let mut diagnostics = Vec::new();

    let readable = match fs::read_dir(&root) {
        Ok(_) => CapabilityProbe::supported("root can be read"),
        Err(e) => CapabilityProbe::unsupported(format!("root cannot be read: {e}")),
    };

    let probe_dir = root.join(format!(".imagedb-capability-probe-{}", Uuid::new_v4()));
    let can_create_dir = match fs::create_dir(&probe_dir) {
        Ok(()) => CapabilityProbe::supported("dedicated probe directory created"),
        Err(e) => CapabilityProbe::unsupported(format!("dedicated probe directory failed: {e}")),
    };

    let writable = if can_create_dir.is_supported() {
        probe_write_file(&probe_dir)
    } else {
        CapabilityProbe::unsupported("probe directory was not created")
    };

    let same_dir_file_rename = if can_create_dir.is_supported() {
        probe_same_dir_file_rename(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let same_root_rename = if can_create_dir.is_supported() {
        probe_same_root_rename(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let directory_rename = if can_create_dir.is_supported() {
        probe_directory_rename(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let overwrite_rename = if can_create_dir.is_supported() {
        probe_overwrite_rename(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let file_sync_all = if can_create_dir.is_supported() {
        probe_file_sync_all(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let parent_dir_sync = if can_create_dir.is_supported() {
        probe_directory_sync(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let case_sensitive = if can_create_dir.is_supported() {
        probe_case_sensitivity(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let unicode_normalization = if can_create_dir.is_supported() {
        probe_unicode_normalization(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let max_component = if can_create_dir.is_supported() {
        probe_max_component(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let max_path = if can_create_dir.is_supported() {
        probe_max_path(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let file_lock = if can_create_dir.is_supported() {
        probe_file_lock(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let timestamp_precision = if can_create_dir.is_supported() {
        probe_timestamp_precision(&probe_dir)
    } else {
        CapabilityProbe::unknown("probe directory was not created")
    };

    let free_space = match fs2::available_space(&root) {
        Ok(bytes) => CapabilityProbe::supported(format!("{bytes} bytes available")),
        Err(e) => CapabilityProbe::unknown(format!("available space could not be read: {e}")),
    };

    let volume_identity = probe_volume_identity(&root);

    let probe_dir_cleaned = cleanup_probe_dir(&probe_dir, &mut diagnostics);

    let (publish_strategy, strategy_reasons) = classify_publish_strategy(PublishStrategyInputs {
        readable: &readable,
        writable: &writable,
        can_create_dir: &can_create_dir,
        same_dir_file_rename: &same_dir_file_rename,
        same_root_rename: &same_root_rename,
        directory_rename: &directory_rename,
        file_sync_all: &file_sync_all,
        parent_dir_sync: &parent_dir_sync,
    });

    StorageCapabilities {
        root: root_display,
        probe_version: 1,
        probed_at: Utc::now(),
        storage_type,
        publish_strategy,
        strategy_reasons,
        probe_dir_cleaned,
        readable,
        writable,
        can_create_dir,
        same_dir_file_rename,
        same_root_rename,
        directory_rename,
        overwrite_rename,
        file_sync_all,
        parent_dir_sync,
        case_sensitive,
        unicode_normalization,
        max_path,
        max_component,
        file_lock,
        timestamp_precision,
        free_space,
        volume_identity,
        diagnostics,
    }
}

pub struct PublishStrategyInputs<'a> {
    pub readable: &'a CapabilityProbe,
    pub writable: &'a CapabilityProbe,
    pub can_create_dir: &'a CapabilityProbe,
    pub same_dir_file_rename: &'a CapabilityProbe,
    pub same_root_rename: &'a CapabilityProbe,
    pub directory_rename: &'a CapabilityProbe,
    pub file_sync_all: &'a CapabilityProbe,
    pub parent_dir_sync: &'a CapabilityProbe,
}

pub fn classify_publish_strategy(
    inputs: PublishStrategyInputs<'_>,
) -> (PublishStrategy, Vec<String>) {
    let required = [
        ("readable", inputs.readable),
        ("writable", inputs.writable),
        ("can_create_dir", inputs.can_create_dir),
        ("same_dir_file_rename", inputs.same_dir_file_rename),
        ("same_root_rename", inputs.same_root_rename),
        ("file_sync_all", inputs.file_sync_all),
    ];

    let missing: Vec<String> = required
        .iter()
        .filter(|(_, probe)| !probe.is_supported())
        .map(|(name, probe)| format!("{name} is {:?}", probe.status))
        .collect();

    if !missing.is_empty() {
        return (PublishStrategy::Unsupported, missing);
    }

    if inputs.directory_rename.is_supported() && inputs.parent_dir_sync.is_supported() {
        return (
            PublishStrategy::StrongLocal,
            vec![
                "directory rename is supported".to_string(),
                "parent directory sync is supported".to_string(),
            ],
        );
    }

    (
        PublishStrategy::ConservativeMounted,
        vec![
            format!("directory_rename is {:?}", inputs.directory_rename.status),
            format!("parent_dir_sync is {:?}", inputs.parent_dir_sync.status),
            "falling back to manifest and commit-marker publish semantics".to_string(),
        ],
    )
}

fn probe_write_file(probe_dir: &Path) -> CapabilityProbe {
    match fs::write(probe_dir.join("write-probe.txt"), b"imagedb") {
        Ok(()) => CapabilityProbe::supported("file can be created and written"),
        Err(e) => CapabilityProbe::unsupported(format!("file write failed: {e}")),
    }
}

fn probe_same_dir_file_rename(probe_dir: &Path) -> CapabilityProbe {
    let from = probe_dir.join("rename-a.txt");
    let to = probe_dir.join("rename-b.txt");
    match fs::write(&from, b"rename").and_then(|_| fs::rename(&from, &to)) {
        Ok(()) => CapabilityProbe::supported("file rename within one directory succeeded"),
        Err(e) => CapabilityProbe::unsupported(format!("same-directory file rename failed: {e}")),
    }
}

fn probe_same_root_rename(probe_dir: &Path) -> CapabilityProbe {
    let a = probe_dir.join("same-root-a");
    let b = probe_dir.join("same-root-b");
    let from = a.join("moved.txt");
    let to = b.join("moved.txt");
    let result = fs::create_dir(&a)
        .and_then(|_| fs::create_dir(&b))
        .and_then(|_| fs::write(&from, b"same-root"))
        .and_then(|_| fs::rename(&from, &to));
    match result {
        Ok(()) => CapabilityProbe::supported("file rename across sibling directories succeeded"),
        Err(e) => CapabilityProbe::unsupported(format!("same-root rename failed: {e}")),
    }
}

fn probe_directory_rename(probe_dir: &Path) -> CapabilityProbe {
    let from = probe_dir.join("dir-rename-a");
    let to = probe_dir.join("dir-rename-b");
    let result = fs::create_dir(&from)
        .and_then(|_| fs::write(from.join("content.txt"), b"dir"))
        .and_then(|_| fs::rename(&from, &to));
    match result {
        Ok(()) => CapabilityProbe::supported("directory rename succeeded"),
        Err(e) => CapabilityProbe::unsupported(format!("directory rename failed: {e}")),
    }
}

fn probe_overwrite_rename(probe_dir: &Path) -> CapabilityProbe {
    let from = probe_dir.join("overwrite-source.txt");
    let to = probe_dir.join("overwrite-target.txt");
    let result = fs::write(&from, b"source")
        .and_then(|_| fs::write(&to, b"target"))
        .and_then(|_| fs::rename(&from, &to));

    match result {
        Ok(()) => match fs::read(&to) {
            Ok(bytes) if bytes == b"source" => {
                CapabilityProbe::supported("rename replaced the existing target")
            }
            Ok(_) => CapabilityProbe::unknown("rename succeeded but target content was unexpected"),
            Err(e) => CapabilityProbe::unknown(format!("rename succeeded but read failed: {e}")),
        },
        Err(e) => CapabilityProbe::unsupported(format!("rename over existing target failed: {e}")),
    }
}

fn probe_file_sync_all(probe_dir: &Path) -> CapabilityProbe {
    let path = probe_dir.join("sync-file.txt");
    let result = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .and_then(|mut file| {
            file.write_all(b"sync")?;
            file.sync_all()
        });

    match result {
        Ok(()) => CapabilityProbe::supported("file sync_all succeeded"),
        Err(e) => CapabilityProbe::unsupported(format!("file sync_all failed: {e}")),
    }
}

fn probe_directory_sync(path: &Path) -> CapabilityProbe {
    match open_directory_for_sync(path).and_then(|dir| dir.sync_all()) {
        Ok(()) => CapabilityProbe::supported("directory sync_all succeeded"),
        Err(e) => {
            CapabilityProbe::unknown(format!("directory sync_all could not be verified: {e}"))
        }
    }
}

#[cfg(windows)]
fn open_directory_for_sync(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

#[cfg(not(windows))]
fn open_directory_for_sync(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

fn probe_case_sensitivity(probe_dir: &Path) -> CapabilityProbe {
    let upper = probe_dir.join("ImageDBCaseProbe.TXT");
    let lower = probe_dir.join("imagedbcaseprobe.txt");
    match fs::write(&upper, b"case") {
        Ok(()) if lower.exists() => {
            CapabilityProbe::unsupported("case variants resolve to the same path")
        }
        Ok(()) => CapabilityProbe::supported("case variants remain distinct"),
        Err(e) => CapabilityProbe::unknown(format!("case sensitivity probe failed: {e}")),
    }
}

fn probe_unicode_normalization(probe_dir: &Path) -> CapabilityProbe {
    let composed = probe_dir.join("unicode-\u{00e9}.txt");
    let decomposed = probe_dir.join("unicode-e\u{0301}.txt");
    let result =
        fs::write(&composed, b"composed").and_then(|_| fs::write(&decomposed, b"decomposed"));

    match result {
        Ok(()) if composed.exists() && decomposed.exists() && composed != decomposed => {
            CapabilityProbe::supported("composed and decomposed Unicode names remain distinct")
        }
        Ok(()) => CapabilityProbe::unsupported("Unicode-equivalent names collapsed"),
        Err(e) => CapabilityProbe::unknown(format!("Unicode normalization probe failed: {e}")),
    }
}

fn probe_max_component(probe_dir: &Path) -> CapabilityProbe {
    let name = format!("{}.txt", "c".repeat(236));
    let path = probe_dir.join(name);
    match fs::write(&path, b"component") {
        Ok(()) => CapabilityProbe::supported("created a 240-character path component"),
        Err(e) => CapabilityProbe::unsupported(format!("240-character component failed: {e}")),
    }
}

fn probe_max_path(probe_dir: &Path) -> CapabilityProbe {
    let mut dir = probe_dir.to_path_buf();
    let mut observed_len = dir.display().to_string().chars().count();

    while observed_len < 270 {
        dir = dir.join("path-depth-segment");
        observed_len = dir.display().to_string().chars().count();
    }

    let file = dir.join("long-path.txt");
    match fs::create_dir_all(&dir).and_then(|_| fs::write(&file, b"long-path")) {
        Ok(()) => CapabilityProbe::supported(format!(
            "created path with {} characters",
            file.display().to_string().chars().count()
        )),
        Err(e) => CapabilityProbe::unsupported(format!(
            "path with at least {observed_len} characters failed: {e}"
        )),
    }
}

fn probe_file_lock(probe_dir: &Path) -> CapabilityProbe {
    let path = probe_dir.join("lock-probe.txt");
    let result = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&path)
        .and_then(|file| {
            file.try_lock_exclusive()?;
            file.unlock()?;
            Ok(())
        });

    match result {
        Ok(()) => CapabilityProbe::supported("exclusive advisory file lock succeeded"),
        Err(e) => {
            CapabilityProbe::unknown(format!("file lock semantics could not be verified: {e}"))
        }
    }
}

fn probe_timestamp_precision(probe_dir: &Path) -> CapabilityProbe {
    let path = probe_dir.join("timestamp-probe.txt");
    let result = fs::write(&path, b"first")
        .and_then(|_| fs::metadata(&path))
        .and_then(|first_meta| {
            std::thread::sleep(Duration::from_millis(25));
            fs::write(&path, b"second")?;
            let second_meta = fs::metadata(&path)?;
            Ok((first_meta.modified()?, second_meta.modified()?))
        });

    match result {
        Ok((first, second)) if second > first => {
            CapabilityProbe::supported("modified timestamp changed after a 25 ms rewrite")
        }
        Ok(_) => {
            CapabilityProbe::unknown("modified timestamp did not change after a 25 ms rewrite")
        }
        Err(e) => CapabilityProbe::unknown(format!("timestamp precision probe failed: {e}")),
    }
}

#[cfg(windows)]
fn probe_volume_identity(root: &Path) -> CapabilityProbe {
    match fs::metadata(root) {
        Ok(_) => CapabilityProbe::unknown(
            "stable Rust cannot read Windows volume identity without platform API binding",
        ),
        Err(e) => CapabilityProbe::unknown(format!("volume identity unavailable: {e}")),
    }
}

#[cfg(not(windows))]
fn probe_volume_identity(root: &Path) -> CapabilityProbe {
    match fs::metadata(root) {
        Ok(meta) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                return CapabilityProbe::supported(format!(
                    "dev={}; ino={}",
                    meta.dev(),
                    meta.ino()
                ));
            }

            #[allow(unreachable_code)]
            CapabilityProbe::unknown("volume identity is not implemented for this platform")
        }
        Err(e) => CapabilityProbe::unknown(format!("volume identity unavailable: {e}")),
    }
}

fn cleanup_probe_dir(probe_dir: &Path, diagnostics: &mut Vec<String>) -> bool {
    if !probe_dir.exists() {
        return false;
    }

    match fs::remove_dir_all(probe_dir) {
        Ok(()) => true,
        Err(e) => {
            diagnostics.push(format!(
                "failed to remove probe directory {}: {e}",
                probe_dir.display()
            ));
            false
        }
    }
}

fn detect_storage_type(path: &Path) -> StorageType {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};

        if matches!(
            path.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::UNC(_, _))
        ) {
            return StorageType::MountedShared;
        }
    }

    StorageType::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn p(status: CapabilityStatus) -> CapabilityProbe {
        CapabilityProbe {
            status,
            detail: String::new(),
        }
    }

    #[test]
    fn probes_temp_dir_and_cleans_up() {
        let temp = TempDir::new().unwrap();
        let capabilities = probe_storage_capabilities(temp.path());

        assert_eq!(capabilities.readable.status, CapabilityStatus::Supported);
        assert_eq!(capabilities.writable.status, CapabilityStatus::Supported);
        assert_eq!(
            capabilities.can_create_dir.status,
            CapabilityStatus::Supported
        );
        assert_eq!(
            capabilities.same_dir_file_rename.status,
            CapabilityStatus::Supported
        );
        assert!(capabilities.probe_dir_cleaned);
        assert!(!fs::read_dir(temp.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".imagedb-capability-probe-")));
    }

    #[test]
    fn missing_root_is_unsupported_without_panic() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("missing");
        let capabilities = probe_storage_capabilities(&missing);

        assert_eq!(capabilities.readable.status, CapabilityStatus::Unsupported);
        assert_eq!(capabilities.publish_strategy, PublishStrategy::Unsupported);
    }

    #[test]
    fn unknown_parent_sync_does_not_classify_as_strong_local() {
        let readable = p(CapabilityStatus::Supported);
        let writable = p(CapabilityStatus::Supported);
        let can_create_dir = p(CapabilityStatus::Supported);
        let same_dir_file_rename = p(CapabilityStatus::Supported);
        let same_root_rename = p(CapabilityStatus::Supported);
        let directory_rename = p(CapabilityStatus::Supported);
        let file_sync_all = p(CapabilityStatus::Supported);
        let parent_dir_sync = p(CapabilityStatus::Unknown);
        let (strategy, reasons) = classify_publish_strategy(PublishStrategyInputs {
            readable: &readable,
            writable: &writable,
            can_create_dir: &can_create_dir,
            same_dir_file_rename: &same_dir_file_rename,
            same_root_rename: &same_root_rename,
            directory_rename: &directory_rename,
            file_sync_all: &file_sync_all,
            parent_dir_sync: &parent_dir_sync,
        });

        assert_eq!(strategy, PublishStrategy::ConservativeMounted);
        assert!(reasons.iter().any(|r| r.contains("parent_dir_sync")));
    }
}

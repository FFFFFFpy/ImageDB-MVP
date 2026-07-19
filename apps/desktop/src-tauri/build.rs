use std::path::{Path, PathBuf};
use std::process::Command;

fn git_output(manifest_dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest_dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn git_path(manifest_dir: &Path, path: &str) -> Option<PathBuf> {
    git_output(
        manifest_dir,
        &["rev-parse", "--path-format=absolute", "--git-path", path],
    )
    .map(PathBuf::from)
}

fn emit_git_rerun_paths(manifest_dir: &Path) {
    let Some(head_path) = git_path(manifest_dir, "HEAD") else {
        return;
    };
    println!("cargo:rerun-if-changed={}", head_path.display());
    if let Some(packed_refs) = git_path(manifest_dir, "packed-refs") {
        println!("cargo:rerun-if-changed={}", packed_refs.display());
    }
    if let Ok(head) = std::fs::read_to_string(&head_path) {
        if let Some(reference) = head.trim().strip_prefix("ref:") {
            if let Some(reference_path) = git_path(manifest_dir, reference.trim()) {
                println!("cargo:rerun-if-changed={}", reference_path.display());
            }
        }
    }
}

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
    println!("cargo:rerun-if-env-changed=IMAGEDB_GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=IMAGEDB_GIT_DIRTY");
    emit_git_rerun_paths(&manifest_dir);

    let commit = std::env::var("IMAGEDB_GIT_COMMIT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| git_output(&manifest_dir, &["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = std::env::var("IMAGEDB_GIT_DIRTY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=IMAGEDB_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=IMAGEDB_GIT_DIRTY={dirty}");
    tauri_build::build()
}

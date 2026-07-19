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

fn git_dirty(manifest_dir: &Path) -> Option<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest_dir)
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .ok()?;
    output.status.success().then_some(!output.stdout.is_empty())
}

fn git_dir(repo_root: &Path) -> Option<PathBuf> {
    let dot_git = repo_root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let pointer = std::fs::read_to_string(dot_git).ok()?;
    let path = pointer.trim().strip_prefix("gitdir:")?.trim();
    let path = PathBuf::from(path);
    Some(if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    })
}

fn emit_tracked_file_rerun_paths(repo_root: &Path) {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["ls-files", "-z"])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    for path in output.stdout.split(|byte| *byte == 0) {
        if path.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(path);
        println!(
            "cargo:rerun-if-changed={}",
            repo_root.join(path.as_ref()).display()
        );
    }
}

fn emit_git_rerun_paths(manifest_dir: &Path) {
    let Some(repo_root) = manifest_dir.ancestors().nth(3) else {
        return;
    };
    let Some(git_dir) = git_dir(repo_root) else {
        return;
    };
    emit_tracked_file_rerun_paths(repo_root);
    let head_path = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head_path.display());
    println!("cargo:rerun-if-changed={}", git_dir.join("index").display());
    if let Ok(head) = std::fs::read_to_string(&head_path) {
        if let Some(reference) = head.trim().strip_prefix("ref:") {
            println!(
                "cargo:rerun-if-changed={}",
                git_dir.join(reference.trim()).display()
            );
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
        .unwrap_or_else(|| {
            git_dirty(&manifest_dir)
                .map(|dirty| dirty.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        });

    println!("cargo:rustc-env=IMAGEDB_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=IMAGEDB_GIT_DIRTY={dirty}");
    tauri_build::build()
}

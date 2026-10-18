//! Deploy-dir scan convention.
//!
//! A Deploy Selection run dir mirrors the reference kit layout: capability
//! dynamic libraries live either at the top level or under `plugins/`, and
//! Title gamepatches under `gamepatchs/`. The loader scans these locations and
//! treats every shared-library file it finds as a capability candidate:
//!
//! - extension `.so` on Linux (target platform),
//! - extension `.dylib` on macOS (host-side development and tests only),
//! - files without those extensions (configs, blobs, catalogs) are ignored.
//!
//! Scan results are sorted by file name for deterministic load order; the
//! descriptor's own metadata decides roles, not the file name.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

/// One discovered capability library file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployEntry {
    pub path: PathBuf,
    /// File name without extension, e.g. `log_stderr` for `liblog_stderr.so`
    /// as well as `log_stderr.so`/`log_stderr.dylib`.
    pub stem: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployScanError {
    /// The deploy dir (or one of its convention subdirs) is not a directory.
    MissingDir(PathBuf),
}

impl std::fmt::Display for DeployScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeployScanError::MissingDir(path) => {
                write!(f, "deploy dir is not a directory: {}", path.display())
            }
        }
    }
}

impl std::error::Error for DeployScanError {}

/// Subdirectories scanned below the deploy root, in order.
pub const SCAN_SUBDIRS: [&str; 2] = ["plugins", "gamepatchs"];

/// Shared-library extensions accepted in a deploy dir. Both are allowed on
/// every host: deployed kits carry Linux `.so` artifacts, while macOS dev
/// builds produce `.dylib`.
pub const LIBRARY_EXTENSIONS: [&str; 2] = ["so", "dylib"];

/// True when `path` looks like a capability library.
pub fn is_capability_library(path: &Path) -> bool {
    path.extension()
        .map(|ext| LIBRARY_EXTENSIONS.contains(&ext.to_string_lossy().as_ref()))
        .unwrap_or(false)
}

/// File name without a leading `lib` prefix and without extension.
pub fn library_stem(file_name: &str) -> String {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    stem.strip_prefix("lib").unwrap_or(stem).to_owned()
}

/// Scan a deploy dir for capability libraries. The root must exist; convention
/// subdirs are scanned only when present. Results are deterministic.
pub fn scan_deploy_dir(deploy_dir: &Path) -> Result<Vec<DeployEntry>, DeployScanError> {
    let metadata = std::fs::metadata(deploy_dir)
        .map_err(|_| DeployScanError::MissingDir(deploy_dir.to_path_buf()))?;
    if !metadata.is_dir() {
        return Err(DeployScanError::MissingDir(deploy_dir.to_path_buf()));
    }

    let mut entries: Vec<DeployEntry> = Vec::new();
    let mut push_dir = |dir: &Path| -> std::io::Result<()> {
        let mut candidates = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if is_capability_library(&path) {
                let file_name = entry.file_name().to_string_lossy().into_owned();
                candidates.push(DeployEntry {
                    path,
                    stem: library_stem(&file_name),
                });
            }
        }
        candidates.sort_by(|a, b| a.stem.cmp(&b.stem).then_with(|| a.path.cmp(&b.path)));
        entries.extend(candidates);
        Ok(())
    };

    push_dir(deploy_dir).map_err(|_| DeployScanError::MissingDir(deploy_dir.to_path_buf()))?;
    for sub in SCAN_SUBDIRS {
        let dir = deploy_dir.join(sub);
        if dir.is_dir() {
            push_dir(&dir).map_err(|_| DeployScanError::MissingDir(dir))?;
        }
    }
    Ok(entries)
}

#[cfg(test)]
#[path = "../tests/unit/scan.rs"]
mod tests;

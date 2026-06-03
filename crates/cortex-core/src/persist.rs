//! Shared persistence helpers: atomic JSON writes and pointer files.
//!
//! These were previously duplicated between `cortex-handoff` and
//! `cortex-core` internals. Centralised here so all crates share one
//! implementation without copy-paste drift.

use std::path::{Path, PathBuf};

use serde::Serialize;
use uuid::Uuid;

use crate::error::{Error, Result};

/// Write `value` as pretty-printed JSON to `target` atomically via a
/// temp file + rename in the same directory. Creates parent directories
/// if they do not exist.
pub fn write_atomic_json<T: Serialize>(target: &Path, value: &T) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| Error::Malformed(format!("no parent dir for {}", target.display())))?;
    std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    let tmp_name = format!(
        "{}.{}.tmp",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("object"),
        Uuid::new_v4().simple()
    );
    let tmp_path = parent.join(tmp_name);
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| Error::json(target, e))?;
    std::fs::write(&tmp_path, &bytes).map_err(|e| Error::io(&tmp_path, e))?;
    if let Err(e) = std::fs::rename(&tmp_path, target) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(Error::io(target, e));
    }
    Ok(())
}

/// Write `filename` to `dir/pointer_name` atomically (temp + rename).
///
/// The pointer file holds the bare filename (not a full path) of the
/// most-recent item in the collection — the same convention used by
/// `cortex-handoff`'s `current` file.
pub fn write_pointer(dir: &Path, pointer_name: &str, filename: &str) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let target = dir.join(pointer_name);
    let tmp_path = dir.join(format!("{pointer_name}.{}.tmp", Uuid::new_v4().simple()));
    std::fs::write(&tmp_path, filename.as_bytes()).map_err(|e| Error::io(&tmp_path, e))?;
    if let Err(e) = std::fs::rename(&tmp_path, &target) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(Error::io(&target, e));
    }
    Ok(())
}

/// Read the contents of `dir/pointer_name`. Returns `None` if the file
/// does not exist or is empty. Errors only on actual I/O failures.
pub fn read_pointer(dir: &Path, pointer_name: &str) -> Result<Option<String>> {
    let path: PathBuf = dir.join(pointer_name);
    if !path.is_file() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
    let trimmed = contents.trim().to_string();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(trimmed))
}

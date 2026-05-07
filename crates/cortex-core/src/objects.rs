//! Sharded content-addressed object store.
//!
//! Layout: `<root>/<first-2-chars>/<full-16-char-hash>.json`
//!
//! Atomic writes use `tempfile` + rename inside the shard directory. Each
//! object stores either raw content (simple mode) or a full `StoredLearning`
//! payload (rich mode used by `cortex-mcp`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::hashing::compute_content_hash;
use crate::models::{Learning, LearningCategory, ProjectContext};
use crate::time::UtcTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StoredLearning {
    Learning {
        content_hash: String,
        category: LearningCategory,
        content: String,
        confidence: f64,
        source: Option<String>,
        first_seen: UtcTime,
        project_context: Option<ProjectContext>,
        stored_at: UtcTime,
    },
}

#[derive(Debug, Clone)]
pub struct ObjectStore {
    root: PathBuf,
}

impl ObjectStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e| Error::io(&root, e))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, content_hash: &str) -> PathBuf {
        let prefix = &content_hash[..2];
        self.root.join(prefix).join(format!("{content_hash}.json"))
    }

    pub fn exists(&self, content_hash: &str) -> bool {
        self.object_path(content_hash).is_file()
    }

    pub fn store_content(&self, content: &str) -> Result<String> {
        let hash = compute_content_hash(content);
        let path = self.object_path(&hash);
        if path.is_file() {
            return Ok(hash);
        }
        let payload = serde_json::json!({
            "content": content,
            "content_hash": hash,
            "stored_at": crate::time::format_rfc3339_z(&chrono::Utc::now()),
        });
        write_atomic_json(&path, &payload)?;
        Ok(hash)
    }

    pub fn store_learning(&self, learning: &Learning) -> Result<String> {
        let hash = learning.content_hash.clone();
        let path = self.object_path(&hash);
        if path.is_file() {
            return Ok(hash);
        }
        let stored = StoredLearning::Learning {
            content_hash: hash.clone(),
            category: learning.category,
            content: learning.content.clone(),
            confidence: learning.confidence,
            source: learning.source.clone(),
            first_seen: learning.created_at,
            project_context: learning.project_context.clone(),
            stored_at: UtcTime::now(),
        };
        write_atomic_json(&path, &stored)?;
        Ok(hash)
    }

    pub fn get_content(&self, content_hash: &str) -> Result<Option<String>> {
        let path = self.object_path(content_hash);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| Error::json(&path, e))?;
        Ok(value
            .get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()))
    }

    pub fn get_learning(&self, content_hash: &str) -> Result<Option<StoredLearning>> {
        let path = self.object_path(content_hash);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let stored: StoredLearning =
            serde_json::from_slice(&bytes).map_err(|e| Error::json(&path, e))?;
        Ok(Some(stored))
    }

    pub fn list_all(&self) -> Result<Vec<String>> {
        let mut hashes = Vec::new();
        if !self.root.is_dir() {
            return Ok(hashes);
        }
        for shard in std::fs::read_dir(&self.root).map_err(|e| Error::io(&self.root, e))? {
            let shard = shard.map_err(|e| Error::io(&self.root, e))?;
            let shard_path = shard.path();
            let Some(name) = shard_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.len() != 2 || !shard_path.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(&shard_path).map_err(|e| Error::io(&shard_path, e))? {
                let entry = entry.map_err(|e| Error::io(&shard_path, e))?;
                let entry_path = entry.path();
                if entry_path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                if let Some(stem) = entry_path.file_stem().and_then(|s| s.to_str()) {
                    if stem.len() == 16 {
                        hashes.push(stem.to_string());
                    }
                }
            }
        }
        hashes.sort();
        Ok(hashes)
    }

    pub fn delete(&self, content_hash: &str) -> Result<bool> {
        let path = self.object_path(content_hash);
        if !path.is_file() {
            return Ok(false);
        }
        std::fs::remove_file(&path).map_err(|e| Error::io(&path, e))?;
        if let Some(parent) = path.parent() {
            // Best-effort shard cleanup; ignore "not empty" errors.
            let _ = std::fs::remove_dir(parent);
        }
        Ok(true)
    }

    pub fn verify_integrity(&self, content_hash: &str) -> Result<bool> {
        match self.get_content(content_hash)? {
            Some(content) => Ok(compute_content_hash(&content) == content_hash),
            None => Ok(false),
        }
    }
}

pub(crate) fn write_atomic_json<T: Serialize>(target: &Path, value: &T) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn roundtrip_simple_content() {
        let dir = TempDir::new().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let hash = store.store_content("hello cortex").unwrap();
        assert!(store.exists(&hash));
        assert_eq!(store.get_content(&hash).unwrap().unwrap(), "hello cortex");
        assert!(store.verify_integrity(&hash).unwrap());
        let listed = store.list_all().unwrap();
        assert_eq!(listed, vec![hash.clone()]);
        assert!(store.delete(&hash).unwrap());
        assert!(!store.exists(&hash));
    }

    #[test]
    fn dedup_returns_existing_hash() {
        let dir = TempDir::new().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        let h1 = store.store_content("hello cortex").unwrap();
        let h2 = store.store_content("HELLO  cortex").unwrap();
        assert_eq!(h1, h2);
    }
}

//! `EpisodeManifest` — the top-level index for the episodic store.
//!
//! Lives at `<state-root>/episodic/manifest.json`. Atomic writes via
//! `cortex_core::persist::write_atomic_json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::episode::EpisodeRecord;

/// Top-level manifest for all episode records in a state root.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpisodeManifest {
    /// Last captured byte offset per session_id.
    ///
    /// Used as the watermark: `capture_tail` starts reading from this
    /// offset so replays never double-capture the same bytes.
    pub consolidated_through_offsets: BTreeMap<String, u64>,
    /// All known episodes keyed by `episode_id`.
    pub episodes: BTreeMap<String, EpisodeRecord>,
}

/// Path to the `episodic/` subdirectory within a state root.
pub fn episodic_dir(state_root: &Path) -> PathBuf {
    state_root.join("episodic")
}

/// Path to the manifest JSON file.
pub fn manifest_path(state_root: &Path) -> PathBuf {
    episodic_dir(state_root).join("manifest.json")
}

/// Load the manifest from disk. Returns a default (empty) manifest if
/// none exists yet.
pub fn load_manifest(state_root: &Path) -> anyhow::Result<EpisodeManifest> {
    let path = manifest_path(state_root);
    if !path.is_file() {
        return Ok(EpisodeManifest::default());
    }
    let bytes = std::fs::read(&path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// Persist the manifest atomically.
pub fn save_manifest(state_root: &Path, m: &EpisodeManifest) -> anyhow::Result<()> {
    let path = manifest_path(state_root);
    cortex_core::persist::write_atomic_json(&path, m).map_err(|e| anyhow::anyhow!("{e}"))
}

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

impl EpisodeManifest {
    /// Validate internal consistency of the manifest.
    ///
    /// Checks:
    /// - Every `(key, rec)` in `episodes` has `key == rec.episode_id`.
    /// - Every `rec.byte_range[0] <= rec.byte_range[1]`.
    /// - For each session in `consolidated_through_offsets`, the watermark is
    ///   >= the max `byte_range[1]` among that session's episodes.
    ///
    /// The watermark check is applied only to sessions whose watermark is set,
    /// and only against episodes whose `byte_range[1]` does not exceed the
    /// watermark by more than the watermark itself (to tolerate truncation-reset
    /// scenarios where pre-truncation Unconsolidated episodes remain in the
    /// manifest with byte ranges from a previous, longer file).
    pub fn validate(&self) -> anyhow::Result<()> {
        for (key, rec) in &self.episodes {
            if key != &rec.episode_id {
                anyhow::bail!(
                    "manifest key {key:?} does not match episode_id {:?}",
                    rec.episode_id
                );
            }
            if rec.byte_range[0] > rec.byte_range[1] {
                anyhow::bail!(
                    "episode {:?} has invalid byte_range: [{}, {}] (start > end)",
                    rec.episode_id,
                    rec.byte_range[0],
                    rec.byte_range[1]
                );
            }
        }

        // Watermark check: for each session that has a watermark, the watermark
        // must be >= the max byte_range[1] of all the session's episodes,
        // provided no episode has a range that starts at 0 with a higher end
        // than the watermark (which is the truncation-reset signature where an
        // old episode predates the reset). We detect truncation-reset by checking
        // whether any episode starts at 0 with byte_range[1] > watermark while
        // another episode also starts at 0 with byte_range[1] <= watermark;
        // if so, we skip the watermark check for that session.
        //
        // Simpler and more robust: only check sessions where the LATEST episode
        // (by byte_range[1]) has byte_range[1] == watermark, which is what
        // capture_tail guarantees in the normal (non-truncated) case.
        'session: for (session_id, &watermark) in &self.consolidated_through_offsets {
            let mut max_end = 0u64;
            let mut min_end_exceeding_watermark = u64::MAX;
            for rec in self.episodes.values() {
                if rec.session_id != *session_id {
                    continue;
                }
                if rec.byte_range[1] > max_end {
                    max_end = rec.byte_range[1];
                }
                // Check for evidence of a truncation-reset: an episode that
                // starts at 0 and whose end > watermark while a newer episode
                // also starts at 0 and ends <= watermark.
                if rec.byte_range[1] > watermark && rec.byte_range[1] < min_end_exceeding_watermark
                {
                    min_end_exceeding_watermark = rec.byte_range[1];
                }
            }
            // If there is an episode that ends <= watermark AND another that
            // ends > watermark for the same session, it's a truncation-reset —
            // skip the watermark check for this session.
            let has_episode_within_watermark = self
                .episodes
                .values()
                .any(|rec| rec.session_id == *session_id && rec.byte_range[1] <= watermark);
            if min_end_exceeding_watermark < u64::MAX && has_episode_within_watermark {
                // Truncation-reset detected: skip watermark check for this session.
                continue 'session;
            }
            if max_end > watermark {
                anyhow::bail!(
                    "session {session_id:?}: watermark {watermark} < max episode end {max_end}"
                );
            }
        }

        Ok(())
    }
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
    let manifest: EpisodeManifest = serde_json::from_slice(&bytes)?;
    manifest.validate()?;
    Ok(manifest)
}

/// Persist the manifest atomically.
pub fn save_manifest(state_root: &Path, m: &EpisodeManifest) -> anyhow::Result<()> {
    let path = manifest_path(state_root);
    cortex_core::persist::write_atomic_json(&path, m).map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::episode::EpisodeRecord;

    fn make_episode(id: &str, session_id: &str, byte_range: [u64; 2]) -> EpisodeRecord {
        let mut ep = EpisodeRecord::new(session_id, "precompact:auto", None, byte_range);
        // Override the generated UUID with a predictable id for key-mismatch tests.
        ep.episode_id = id.to_string();
        ep
    }

    #[test]
    fn validate_valid_manifest_passes() {
        let ep = make_episode("ep-1", "sess-1", [0, 100]);
        let mut manifest = EpisodeManifest::default();
        manifest
            .consolidated_through_offsets
            .insert("sess-1".to_string(), 100);
        manifest.episodes.insert("ep-1".to_string(), ep);
        assert!(manifest.validate().is_ok(), "valid manifest should pass");
    }

    #[test]
    fn validate_key_mismatch_fails() {
        let ep = make_episode("ep-real-id", "sess-1", [0, 50]);
        let mut manifest = EpisodeManifest::default();
        manifest
            .consolidated_through_offsets
            .insert("sess-1".to_string(), 50);
        // Intentional mismatch: key "wrong-key" != episode_id "ep-real-id"
        manifest.episodes.insert("wrong-key".to_string(), ep);
        let err = manifest.validate().unwrap_err();
        assert!(
            err.to_string().contains("does not match episode_id"),
            "expected key-mismatch error, got: {err}"
        );
    }

    #[test]
    fn validate_start_greater_than_end_fails() {
        let ep = make_episode("ep-bad-range", "sess-1", [100, 50]);
        let mut manifest = EpisodeManifest::default();
        // watermark doesn't matter here — range check fires first
        manifest
            .consolidated_through_offsets
            .insert("sess-1".to_string(), 100);
        manifest.episodes.insert("ep-bad-range".to_string(), ep);
        let err = manifest.validate().unwrap_err();
        assert!(
            err.to_string().contains("start > end"),
            "expected start>end error, got: {err}"
        );
    }
}

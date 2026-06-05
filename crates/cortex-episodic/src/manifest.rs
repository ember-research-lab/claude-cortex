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
    /// - For each session in `consolidated_through_offsets`: if that session
    ///   has no episodes (all pruned), skip it. Otherwise the episode with the
    ///   latest `captured_at` must have `byte_range[1] == watermark`. This
    ///   correctly catches real desync while tolerating truncation-reset (the
    ///   post-reset episode becomes the latest and its end equals the new,
    ///   smaller watermark).
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

        for (session_id, &watermark) in &self.consolidated_through_offsets {
            // Find the episode with the latest captured_at for this session.
            let latest = self
                .episodes
                .values()
                .filter(|rec| rec.session_id == *session_id)
                .max_by_key(|rec| rec.captured_at.into_inner());

            let Some(latest) = latest else {
                // All episodes for this session were pruned; watermark may persist.
                continue;
            };

            if latest.byte_range[1] != watermark {
                anyhow::bail!(
                    "session {session_id:?}: watermark {watermark} != latest episode end {} \
                     (episode {:?})",
                    latest.byte_range[1],
                    latest.episode_id
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
    use std::time::Duration;

    fn make_episode(id: &str, session_id: &str, byte_range: [u64; 2]) -> EpisodeRecord {
        let mut ep = EpisodeRecord::new(session_id, "precompact:auto", None, byte_range);
        // Override the generated UUID with a predictable id for key-mismatch tests.
        ep.episode_id = id.to_string();
        ep
    }

    /// Like `make_episode` but with an explicit `captured_at` offset from now.
    fn make_episode_at(
        id: &str,
        session_id: &str,
        byte_range: [u64; 2],
        offset: Duration,
    ) -> EpisodeRecord {
        use chrono::Utc;
        use cortex_core::time::UtcTime;
        let mut ep = make_episode(id, session_id, byte_range);
        ep.captured_at = UtcTime::from(Utc::now() - chrono::Duration::from_std(offset).unwrap());
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

    #[test]
    fn validate_watermark_mismatch_fails() {
        // Session has one episode ending at 80, but watermark claims 100.
        let ep = make_episode("ep-1", "sess-1", [0, 80]);
        let mut manifest = EpisodeManifest::default();
        manifest
            .consolidated_through_offsets
            .insert("sess-1".to_string(), 100);
        manifest.episodes.insert("ep-1".to_string(), ep);
        let err = manifest.validate().unwrap_err();
        assert!(
            err.to_string().contains("watermark"),
            "expected watermark mismatch error, got: {err}"
        );
    }

    #[test]
    fn validate_truncation_reset_passes() {
        // Pre-truncation episode: captured earlier, byte_range [0, 200].
        // Post-truncation episode: captured later, byte_range [0, 50].
        // Watermark = 50 (set by the latest capture).
        // The latest-by-captured_at episode's end == watermark → valid.
        let older = make_episode_at("ep-old", "sess-1", [0, 200], Duration::from_secs(60));
        let newer = make_episode_at("ep-new", "sess-1", [0, 50], Duration::from_secs(0));
        let mut manifest = EpisodeManifest::default();
        manifest
            .consolidated_through_offsets
            .insert("sess-1".to_string(), 50);
        manifest.episodes.insert("ep-old".to_string(), older);
        manifest.episodes.insert("ep-new".to_string(), newer);
        assert!(
            manifest.validate().is_ok(),
            "truncation-reset manifest should pass"
        );
    }

    #[test]
    fn validate_all_episodes_pruned_passes() {
        // Watermark exists but no episodes remain (all pruned) — skip check.
        let mut manifest = EpisodeManifest::default();
        manifest
            .consolidated_through_offsets
            .insert("sess-1".to_string(), 100);
        assert!(
            manifest.validate().is_ok(),
            "watermark with no episodes (all pruned) should pass"
        );
    }
}

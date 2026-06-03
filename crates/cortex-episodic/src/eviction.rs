//! Eviction logic: mark episodes as `Evictable` once their promoted blocks
//! are confirmed or the TTL has elapsed.
//!
//! `reconcile_eviction` is a **pure function** — it takes a manifest and
//! reinforcements and returns an updated manifest without touching the
//! filesystem. The caller is responsible for persisting the result.

use std::path::Path;

use chrono::Utc;
use cortex_core::models::{OutcomeResult, Reinforcements};

use crate::episode::{EpisodeRecord, EpisodeStatus};
use crate::manifest::{episodic_dir, save_manifest, EpisodeManifest};

/// Pure: mark episodes as `Evictable` when:
/// - All `promoted_block_ids` have at least one `Success` outcome in the
///   reinforcements ledger, OR
/// - The episode is older than `ttl_days` (TTL backstop).
///
/// Only episodes in the `ConsolidatedPendingConfirmation` state are
/// eligible. `Unconsolidated`, `Consolidating`, and already-`Evictable`
/// episodes are left unchanged.
///
/// Returns the updated manifest; does NOT write it.
pub fn reconcile_eviction(
    mut manifest: EpisodeManifest,
    reinforcements: &Reinforcements,
    ttl_days: u32,
) -> EpisodeManifest {
    let now = Utc::now();
    let ttl = chrono::Duration::days(ttl_days as i64);

    for episode in manifest.episodes.values_mut() {
        if episode.status != EpisodeStatus::ConsolidatedPendingConfirmation {
            continue;
        }

        // TTL backstop: evict regardless of outcome confirmation.
        let age = now.signed_duration_since(episode.captured_at.into_inner());
        if age >= ttl {
            episode.status = EpisodeStatus::Evictable;
            continue;
        }

        // Outcome confirmation: all promoted blocks must have a Success outcome.
        if !episode.promoted_block_ids.is_empty() && all_blocks_confirmed(episode, reinforcements) {
            episode.status = EpisodeStatus::Evictable;
        }
    }

    manifest
}

/// Returns true when every block id in `episode.promoted_block_ids` has at
/// least one `Success` outcome in the reinforcements ledger.
///
/// The reinforcements ledger is keyed by learning id; each entry carries a
/// `block_id` field. We scan all reinforcements looking for entries whose
/// `block_id` matches a promoted id, then check their outcomes.
fn all_blocks_confirmed(episode: &EpisodeRecord, reinforcements: &Reinforcements) -> bool {
    episode.promoted_block_ids.iter().all(|block_id| {
        reinforcements
            .learnings
            .values()
            .filter(|r| &r.block_id == block_id)
            .any(|r| {
                r.outcomes
                    .iter()
                    .any(|o| o.result == OutcomeResult::Success)
            })
    })
}

/// Prune episode files for all `Evictable` episodes, remove them from the
/// manifest, and persist the updated manifest. Returns the number of
/// episodes pruned.
pub fn prune_evictable(state_root: &Path, manifest: &mut EpisodeManifest) -> anyhow::Result<usize> {
    let dir = episodic_dir(state_root);
    let evictable_ids: Vec<String> = manifest
        .episodes
        .values()
        .filter(|e| e.status == EpisodeStatus::Evictable)
        .map(|e| e.episode_id.clone())
        .collect();

    let mut pruned = 0;
    for id in &evictable_ids {
        if let Some(episode) = manifest.episodes.remove(id) {
            let path = dir.join(episode.filename());
            if path.is_file() {
                std::fs::remove_file(&path)?;
            }
            pruned += 1;
        }
    }

    if pruned > 0 {
        save_manifest(state_root, manifest)?;
    }

    Ok(pruned)
}

//! Phase 5 gate tests for `cortex-episodic`.
//!
//! These tests exercise `prune_evictable` on disk — distinct from Phase 1's
//! pure `reconcile_eviction` tests which never touch the filesystem.

use chrono::Utc;
use cortex_core::models::{
    LearningCategory, OutcomeResult, Reinforcement, ReinforcementOutcome, Reinforcements,
};
use cortex_core::time::UtcTime;
use cortex_episodic::{
    episodic_dir, load_manifest, prune_evictable, reconcile_eviction, save_manifest,
    EpisodeManifest, EpisodeRecord, EpisodeStatus,
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an EpisodeRecord in `ConsolidatedPendingConfirmation` state,
/// write it to disk in `state_root/episodic/`, and insert it into `manifest`.
fn seed_consolidated_episode(
    state_root: &std::path::Path,
    manifest: &mut EpisodeManifest,
    session_id: &str,
    block_ids: Vec<String>,
    days_ago: i64,
) -> EpisodeRecord {
    let mut ep = EpisodeRecord::new(session_id, "precompact:auto", None, [0, 0]);
    ep.promoted_block_ids = block_ids;
    ep.status = EpisodeStatus::ConsolidatedPendingConfirmation;
    // Backdate captured_at to simulate age.
    let past = Utc::now() - chrono::Duration::days(days_ago);
    ep.captured_at = UtcTime::from(past);

    let dir = episodic_dir(state_root);
    std::fs::create_dir_all(&dir).unwrap();
    let episode_path = dir.join(ep.filename());
    cortex_core::persist::write_atomic_json(&episode_path, &ep).unwrap();

    manifest.episodes.insert(ep.episode_id.clone(), ep.clone());

    ep
}

/// Build a `Reinforcements` where `block_id` has exactly one Success outcome.
fn reinforcements_with_success(block_id: &str) -> Reinforcements {
    let now = UtcTime::now();
    let reinforcement = Reinforcement {
        category: LearningCategory::Discovery,
        content: "test content".to_string(),
        confidence: 0.9,
        outcome_count: 1,
        last_updated: now,
        last_applied: now,
        block_id: block_id.to_string(),
        content_hash: "abc123".to_string(),
        object_store_hash: "def456".to_string(),
        outcomes: vec![ReinforcementOutcome {
            timestamp: now,
            result: OutcomeResult::Success,
            context: "confirmed".to_string(),
            delta: 0.1,
        }],
        origin: cortex_core::models::Origin::default(),
        corroboration: 0,
    };
    let mut r = Reinforcements::default();
    r.learnings
        .insert("learning-id-1".to_string(), reinforcement);
    r
}

// ---------------------------------------------------------------------------
// Test 1: episode_not_pruned_before_blocks_confirmed
// ---------------------------------------------------------------------------

#[test]
fn episode_not_pruned_before_blocks_confirmed() {
    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path();

    let mut manifest = EpisodeManifest::default();
    let ep = seed_consolidated_episode(
        state_root,
        &mut manifest,
        "session-pending",
        vec!["block-X".to_string()],
        5, // 5 days ago — well within TTL=30
    );
    save_manifest(state_root, &manifest).unwrap();

    // No outcomes for block-X.
    let reinforcements = Reinforcements::default();

    let manifest = reconcile_eviction(manifest, &reinforcements, 30);

    // Episode must still be ConsolidatedPendingConfirmation.
    assert_eq!(
        manifest.episodes[&ep.episode_id].status,
        EpisodeStatus::ConsolidatedPendingConfirmation,
        "episode should remain ConsolidatedPendingConfirmation when block-X has no outcomes"
    );

    // prune_evictable should remove 0 files and 0 manifest entries.
    let mut manifest_mut = manifest;
    let pruned = prune_evictable(state_root, &mut manifest_mut).unwrap();
    assert_eq!(pruned, 0, "prune_evictable should remove 0 episodes");
    assert!(
        manifest_mut.episodes.contains_key(&ep.episode_id),
        "episode should still be in manifest after no-op prune"
    );

    // Episode file must still exist on disk.
    let episode_path = episodic_dir(state_root).join(ep.filename());
    assert!(
        episode_path.is_file(),
        "episode file should still exist on disk before confirmation"
    );
}

// ---------------------------------------------------------------------------
// Test 2: episode_pruned_after_success_outcome
// ---------------------------------------------------------------------------

#[test]
fn episode_pruned_after_success_outcome() {
    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path();

    let mut manifest = EpisodeManifest::default();
    let ep = seed_consolidated_episode(
        state_root,
        &mut manifest,
        "session-success",
        vec!["block-X".to_string()],
        5, // within TTL=30
    );
    save_manifest(state_root, &manifest).unwrap();

    // block-X has a Success outcome.
    let reinforcements = reinforcements_with_success("block-X");

    let manifest = reconcile_eviction(manifest, &reinforcements, 30);

    // reconcile must mark the episode Evictable.
    assert_eq!(
        manifest.episodes[&ep.episode_id].status,
        EpisodeStatus::Evictable,
        "episode should be Evictable after block-X Success outcome"
    );

    let episode_file = episodic_dir(state_root).join(ep.filename());
    assert!(
        episode_file.is_file(),
        "episode file should still exist before prune"
    );

    // prune_evictable must remove the file and the manifest entry.
    let mut manifest_mut = manifest;
    let pruned = prune_evictable(state_root, &mut manifest_mut).unwrap();
    assert_eq!(pruned, 1, "prune_evictable should remove 1 episode");
    assert!(
        !manifest_mut.episodes.contains_key(&ep.episode_id),
        "episode should be removed from manifest after prune"
    );
    assert!(
        !episode_file.is_file(),
        "episode file should be deleted from disk after prune"
    );
}

// ---------------------------------------------------------------------------
// Test 3: episode_pruned_after_ttl_regardless_of_outcomes
// ---------------------------------------------------------------------------

#[test]
fn episode_pruned_after_ttl_regardless_of_outcomes() {
    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path();

    let mut manifest = EpisodeManifest::default();
    let ep = seed_consolidated_episode(
        state_root,
        &mut manifest,
        "session-ttl",
        vec!["block-X".to_string()],
        31, // 31 days ago — exceeds TTL=30
    );
    save_manifest(state_root, &manifest).unwrap();

    // No outcomes for block-X (TTL alone should trigger eviction).
    let reinforcements = Reinforcements::default();

    let manifest = reconcile_eviction(manifest, &reinforcements, 30);

    assert_eq!(
        manifest.episodes[&ep.episode_id].status,
        EpisodeStatus::Evictable,
        "episode should be Evictable after TTL elapsed regardless of outcomes"
    );

    let episode_file = episodic_dir(state_root).join(ep.filename());
    let mut manifest_mut = manifest;
    let pruned = prune_evictable(state_root, &mut manifest_mut).unwrap();
    assert_eq!(
        pruned, 1,
        "prune_evictable should remove 1 episode after TTL"
    );
    assert!(
        !manifest_mut.episodes.contains_key(&ep.episode_id),
        "episode should be removed from manifest after TTL eviction"
    );
    assert!(
        !episode_file.is_file(),
        "episode file should be deleted from disk after TTL eviction"
    );
}

// ---------------------------------------------------------------------------
// Test (I2): empty_promotion_is_evictable
// ---------------------------------------------------------------------------

#[test]
fn empty_promotion_is_evictable() {
    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path();

    let mut manifest = EpisodeManifest::default();
    // Episode with NO promoted_block_ids — nothing to confirm.
    let ep = seed_consolidated_episode(
        state_root,
        &mut manifest,
        "session-empty-promo",
        vec![], // empty
        5,      // within TTL=30
    );
    save_manifest(state_root, &manifest).unwrap();

    let reinforcements = Reinforcements::default();
    let manifest = reconcile_eviction(manifest, &reinforcements, 30);

    assert_eq!(
        manifest.episodes[&ep.episode_id].status,
        EpisodeStatus::Evictable,
        "episode with empty promoted_block_ids must be immediately Evictable"
    );

    let episode_file = episodic_dir(state_root).join(ep.filename());
    let mut manifest_mut = manifest;
    let pruned = prune_evictable(state_root, &mut manifest_mut).unwrap();
    assert_eq!(pruned, 1, "prune_evictable should remove 1 episode");
    assert!(
        !manifest_mut.episodes.contains_key(&ep.episode_id),
        "episode should be removed from manifest after prune"
    );
    assert!(
        !episode_file.is_file(),
        "episode file should be deleted from disk after prune"
    );
}

// ---------------------------------------------------------------------------
// Test 4: episode_retrievable_before_eviction
// ---------------------------------------------------------------------------

#[test]
fn episode_retrievable_before_eviction() {
    let tmp = TempDir::new().unwrap();
    let state_root = tmp.path();

    let mut manifest = EpisodeManifest::default();
    let ep = seed_consolidated_episode(
        state_root,
        &mut manifest,
        "session-retrievable",
        vec!["block-Y".to_string()],
        5, // within TTL — not yet evictable
    );
    save_manifest(state_root, &manifest).unwrap();

    // Assert the episode file exists on disk.
    let episode_file = episodic_dir(state_root).join(ep.filename());
    assert!(
        episode_file.is_file(),
        "episode file must exist on disk before any eviction"
    );

    // Assert the file can be deserialized back to EpisodeRecord.
    let raw = std::fs::read(&episode_file).unwrap();
    let loaded: EpisodeRecord =
        serde_json::from_slice(&raw).expect("episode file must deserialize to EpisodeRecord");
    assert_eq!(
        loaded.episode_id, ep.episode_id,
        "round-tripped episode_id must match"
    );
    assert_eq!(
        loaded.status,
        EpisodeStatus::ConsolidatedPendingConfirmation,
        "round-tripped status must be ConsolidatedPendingConfirmation"
    );
    assert_eq!(
        loaded.promoted_block_ids,
        vec!["block-Y".to_string()],
        "round-tripped promoted_block_ids must match"
    );

    // Confirm the loaded manifest also references this episode.
    let on_disk_manifest = load_manifest(state_root).unwrap();
    assert!(
        on_disk_manifest.episodes.contains_key(&ep.episode_id),
        "manifest must reference the episode before eviction runs"
    );
}

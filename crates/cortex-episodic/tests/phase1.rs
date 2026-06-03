//! Phase 1 gate tests for `cortex-episodic`.
//!
//! All 7 tests must pass before Phase 2 work begins.

use std::io::Write as IoWrite;

use chrono::Utc;
use cortex_core::models::{
    LearningCategory, OutcomeResult, Reinforcement, ReinforcementOutcome, Reinforcements,
};
use cortex_core::time::UtcTime;
use cortex_episodic::{
    capture_tail, load_manifest, reconcile_eviction, EpisodeManifest, EpisodeRecord, EpisodeStatus,
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_transcript(dir: &TempDir, content: &[u8]) -> String {
    let path = dir.path().join("transcript.jsonl");
    std::fs::write(&path, content).unwrap();
    path.to_str().unwrap().to_string()
}

fn append_transcript(path: &str, extra: &[u8]) {
    let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    f.write_all(extra).unwrap();
}

// ---------------------------------------------------------------------------
// Test 1: round_trip_capture_writes_correct_byte_range
// ---------------------------------------------------------------------------

#[test]
fn round_trip_capture_writes_correct_byte_range() {
    let state_dir = TempDir::new().unwrap();
    let content = vec![b'x'; 200];
    let transcript = make_transcript(&state_dir, &content);

    let episode = capture_tail(
        state_dir.path(),
        "session-1",
        Some(&transcript),
        "precompact:auto",
        None,
    )
    .unwrap()
    .expect("should return Some");

    assert_eq!(
        episode.byte_range,
        [0, 200],
        "byte range should be [0, 200]"
    );
    assert!(
        !episode.episode_id.is_empty(),
        "episode_id must be non-empty"
    );

    // Manifest watermark must be advanced to 200.
    let manifest = load_manifest(state_dir.path()).unwrap();
    assert_eq!(
        manifest
            .consolidated_through_offsets
            .get("session-1")
            .copied(),
        Some(200),
        "manifest watermark should be 200"
    );

    // Exactly one episode in manifest.
    assert_eq!(manifest.episodes.len(), 1);

    // Episode file must exist on disk.
    let episode_path = state_dir.path().join("episodic").join(episode.filename());
    assert!(episode_path.is_file(), "episode file should exist on disk");
}

// ---------------------------------------------------------------------------
// Test 2: second_capture_at_same_watermark_is_noop
// ---------------------------------------------------------------------------

#[test]
fn second_capture_at_same_watermark_is_noop() {
    let state_dir = TempDir::new().unwrap();
    let content = vec![b'y'; 200];
    let transcript = make_transcript(&state_dir, &content);

    // First capture.
    capture_tail(
        state_dir.path(),
        "session-2",
        Some(&transcript),
        "precompact:auto",
        None,
    )
    .unwrap()
    .expect("first capture should succeed");

    // Second capture with same watermark (file unchanged).
    let result = capture_tail(
        state_dir.path(),
        "session-2",
        Some(&transcript),
        "precompact:auto",
        None,
    )
    .unwrap();

    assert!(result.is_none(), "second capture should return None");

    let manifest = load_manifest(state_dir.path()).unwrap();
    assert_eq!(
        manifest.episodes.len(),
        1,
        "manifest should still have exactly 1 episode"
    );
}

// ---------------------------------------------------------------------------
// Test 3: capture_tail_advances_watermark_on_append
// ---------------------------------------------------------------------------

#[test]
fn capture_tail_advances_watermark_on_append() {
    let state_dir = TempDir::new().unwrap();
    let content = vec![b'z'; 200];
    let transcript = make_transcript(&state_dir, &content);

    // First capture.
    let ep1 = capture_tail(
        state_dir.path(),
        "session-3",
        Some(&transcript),
        "precompact:auto",
        None,
    )
    .unwrap()
    .expect("first capture should succeed");
    assert_eq!(ep1.byte_range, [0, 200]);

    // Append 50 more bytes.
    append_transcript(&transcript, &[b'a'; 50]);

    // Second capture.
    let ep2 = capture_tail(
        state_dir.path(),
        "session-3",
        Some(&transcript),
        "precompact:auto",
        None,
    )
    .unwrap()
    .expect("second capture should succeed");

    assert_eq!(
        ep2.byte_range,
        [200, 250],
        "second episode should cover bytes [200, 250]"
    );

    let manifest = load_manifest(state_dir.path()).unwrap();
    assert_eq!(
        manifest.episodes.len(),
        2,
        "manifest should have 2 episodes"
    );
    assert_eq!(
        manifest
            .consolidated_through_offsets
            .get("session-3")
            .copied(),
        Some(250)
    );
}

// ---------------------------------------------------------------------------
// Test 4: capture_without_transcript_path_writes_zero_range
// ---------------------------------------------------------------------------

#[test]
fn capture_without_transcript_path_writes_zero_range() {
    let state_dir = TempDir::new().unwrap();

    let episode = capture_tail(
        state_dir.path(),
        "session-4",
        None, // no transcript path
        "sessionend:stop",
        None,
    )
    .unwrap()
    .expect("should return Some even without transcript");

    assert_eq!(
        episode.byte_range,
        [0, 0],
        "byte_range should be [0, 0] when no transcript"
    );
    assert!(episode.transcript_path.is_none());
}

// ---------------------------------------------------------------------------
// Helpers for eviction tests
// ---------------------------------------------------------------------------

fn make_episode_with_status(
    session_id: &str,
    block_ids: Vec<String>,
    status: EpisodeStatus,
    days_ago: i64,
) -> EpisodeRecord {
    let mut ep = EpisodeRecord::new(session_id, "precompact:auto", None, [0, 0]);
    ep.promoted_block_ids = block_ids;
    ep.status = status;
    // Backdate captured_at.
    let past = Utc::now() - chrono::Duration::days(days_ago);
    ep.captured_at = cortex_core::time::UtcTime::from(past);
    ep
}

fn make_reinforcements_with_success(block_id: &str) -> Reinforcements {
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
    };
    let mut r = Reinforcements::default();
    r.learnings
        .insert("learning-id-1".to_string(), reinforcement);
    r
}

fn make_empty_reinforcements() -> Reinforcements {
    Reinforcements::default()
}

// ---------------------------------------------------------------------------
// Test 5: reconcile_eviction_pure_function
// ---------------------------------------------------------------------------

#[test]
fn reconcile_eviction_pure_function() {
    // Episode A: block-A has a Success outcome → should become Evictable.
    let ep_a = make_episode_with_status(
        "s-a",
        vec!["block-A".to_string()],
        EpisodeStatus::ConsolidatedPendingConfirmation,
        5, // 5 days ago — within TTL
    );
    // Episode B: block-B has no outcomes, 5 days ago — within TTL.
    let ep_b = make_episode_with_status(
        "s-b",
        vec!["block-B".to_string()],
        EpisodeStatus::ConsolidatedPendingConfirmation,
        5,
    );

    let mut manifest = EpisodeManifest::default();
    manifest
        .episodes
        .insert(ep_a.episode_id.clone(), ep_a.clone());
    manifest
        .episodes
        .insert(ep_b.episode_id.clone(), ep_b.clone());

    // Reinforcements: block-A is confirmed, block-B is not.
    let reinforcements = make_reinforcements_with_success("block-A");

    let updated = reconcile_eviction(manifest, &reinforcements, 30);

    assert_eq!(
        updated.episodes[&ep_a.episode_id].status,
        EpisodeStatus::Evictable,
        "block-A episode should be Evictable after Success outcome"
    );
    assert_eq!(
        updated.episodes[&ep_b.episode_id].status,
        EpisodeStatus::ConsolidatedPendingConfirmation,
        "block-B episode should remain ConsolidatedPendingConfirmation"
    );
}

// ---------------------------------------------------------------------------
// Test 6: reconcile_eviction_ttl_backstop
// ---------------------------------------------------------------------------

#[test]
fn reconcile_eviction_ttl_backstop() {
    // Episode B: no outcomes, captured 31 days ago with TTL=30 → evictable via TTL.
    let ep_b = make_episode_with_status(
        "s-b",
        vec!["block-B".to_string()],
        EpisodeStatus::ConsolidatedPendingConfirmation,
        31, // exceeds TTL=30
    );
    // Episode A: Success outcome, 5 days ago.
    let ep_a = make_episode_with_status(
        "s-a",
        vec!["block-A".to_string()],
        EpisodeStatus::ConsolidatedPendingConfirmation,
        5,
    );

    let mut manifest = EpisodeManifest::default();
    manifest
        .episodes
        .insert(ep_a.episode_id.clone(), ep_a.clone());
    manifest
        .episodes
        .insert(ep_b.episode_id.clone(), ep_b.clone());

    let reinforcements = make_reinforcements_with_success("block-A");

    let updated = reconcile_eviction(manifest, &reinforcements, 30);

    assert_eq!(
        updated.episodes[&ep_a.episode_id].status,
        EpisodeStatus::Evictable,
        "block-A evictable via Success outcome"
    );
    assert_eq!(
        updated.episodes[&ep_b.episode_id].status,
        EpisodeStatus::Evictable,
        "block-B evictable via TTL backstop"
    );
}

// ---------------------------------------------------------------------------
// Test 7: eviction_never_prunes_unconsolidated
// ---------------------------------------------------------------------------

#[test]
fn eviction_never_prunes_unconsolidated() {
    // Unconsolidated episode older than TTL — must NOT become Evictable.
    let ep = make_episode_with_status(
        "s-unc",
        vec!["block-X".to_string()],
        EpisodeStatus::Unconsolidated,
        60, // well past TTL=30
    );

    let mut manifest = EpisodeManifest::default();
    manifest.episodes.insert(ep.episode_id.clone(), ep.clone());

    let reinforcements = make_empty_reinforcements();
    let updated = reconcile_eviction(manifest, &reinforcements, 30);

    assert_eq!(
        updated.episodes[&ep.episode_id].status,
        EpisodeStatus::Unconsolidated,
        "Unconsolidated episodes must never be marked Evictable"
    );
}

//! End-to-end test of the cortex-dream pipeline against a real seeded
//! v3 ledger. Verifies all 6 stages: read → BM25 → graph → eigendecomp
//! → active memory → spectrum snapshot.

use cortex_active_memory::{active_memory_dir, current_pointer_path, read_current};
use cortex_core::models::{Identity, LearningCategory};
use cortex_core::{Learning, Ledger};
use cortex_dream::run;
use cortex_monitor::{load_history, spectrum_history_dir};
use std::path::Path;
use tempfile::TempDir;

fn seed_ledger(dir: &Path, learnings: Vec<(LearningCategory, &str)>) {
    let ledger = Ledger::open(dir).unwrap();
    ledger
        .key_manager()
        .generate_keypair(&Identity {
            name: "dream-test".into(),
            machine: "ci".into(),
            email: None,
        })
        .unwrap();
    let block_learnings: Vec<_> = learnings
        .into_iter()
        .map(|(cat, content)| Learning::new(cat, content, 0.7, None))
        .collect();
    ledger.append_block("seed", block_learnings, true).unwrap();
}

#[test]
fn dream_pipeline_runs_end_to_end() {
    let project = TempDir::new().unwrap();
    let ledger_dir = project.path().join("ledger");
    let state_dir = project.path().join("state");

    seed_ledger(
        &ledger_dir,
        vec![
            (
                LearningCategory::Pattern,
                "atomic writes use tempfile and rename inside a flock-held parent",
            ),
            (
                LearningCategory::Discovery,
                "v3 substrate stores RFC3339 timestamps in canonical Z form",
            ),
            (
                LearningCategory::Decision,
                "match v2 SHA-256 hashing instead of switching to BLAKE3",
            ),
            (
                LearningCategory::Pattern,
                "BM25 lexical similarity over short ledger entries beats embeddings at small scale",
            ),
            (
                LearningCategory::Discovery,
                "Windows fs2 LockFileEx is mandatory locking; lock a sentinel sibling instead",
            ),
        ],
    );

    let report = run(&ledger_dir, &state_dir, None).unwrap();

    // Every learning becomes a node in the graph.
    assert_eq!(report.n_nodes, 5);
    // 5 nodes → top-k = min(50, 5/3) = 1.
    assert_eq!(report.k, 1);
    // Some BM25 similarity should fire across the 5 entries (tempfile,
    // ledger / substrate vocabulary, hashing); expect at least one edge.
    assert!(
        report.n_edges > 0,
        "expected non-zero edges from BM25 similarity"
    );
    // Active memory should be non-empty and bounded by k.
    assert!(report.entries > 0);
    assert!(report.entries <= report.k);

    // Active snapshot file exists.
    assert!(report.active_snapshot.is_file());
    // current pointer exists and points at a real snapshot.
    assert!(current_pointer_path(&state_dir).is_file());
    let active = read_current(&state_dir).unwrap().expect("no current");
    assert_eq!(active.entries.len(), report.entries);
    // Each entry has a learning_id and projection_weight.
    for entry in &active.entries {
        assert!(!entry.learning_id.is_empty());
        assert!(entry.projection_weight.is_finite());
    }

    // Spectrum snapshot recorded under spectrum-history.
    assert!(report.spectrum_snapshot.is_file());
    let history = load_history(&state_dir).unwrap();
    assert_eq!(history.snapshots.len(), 1);
    assert_eq!(history.snapshots[0].n_nodes, 5);
}

#[test]
fn dream_on_empty_ledger_produces_empty_snapshot() {
    let project = TempDir::new().unwrap();
    let ledger_dir = project.path().join("ledger");
    let state_dir = project.path().join("state");
    Ledger::open(&ledger_dir).unwrap();

    let report = run(&ledger_dir, &state_dir, None).unwrap();
    assert_eq!(report.n_nodes, 0);
    assert_eq!(report.entries, 0);
    // No graph means an empty active memory snapshot — but the snapshot
    // file should still exist (the dreaming pass always emits one) so
    // cortex-monitor history accumulates a "ledger was empty here" entry.
    assert!(report.active_snapshot.is_file());
}

#[test]
fn two_dream_runs_produce_two_spectrum_snapshots() {
    let project = TempDir::new().unwrap();
    let ledger_dir = project.path().join("ledger");
    let state_dir = project.path().join("state");

    seed_ledger(
        &ledger_dir,
        vec![
            (
                LearningCategory::Pattern,
                "first pattern in the seeded corpus",
            ),
            (
                LearningCategory::Discovery,
                "second discovery in the seeded corpus",
            ),
        ],
    );

    // First pass.
    let r1 = run(&ledger_dir, &state_dir, None).unwrap();
    // Need a different timestamp for the second pass — wait long enough
    // that the microsecond-resolution timestamps differ. Sleeping 1ms is
    // sufficient given format precision.
    std::thread::sleep(std::time::Duration::from_millis(2));
    let r2 = run(&ledger_dir, &state_dir, None).unwrap();
    assert_ne!(r1.active_snapshot, r2.active_snapshot);

    let active_files = std::fs::read_dir(active_memory_dir(&state_dir))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("active-") && n.ends_with(".json"))
        })
        .count();
    assert_eq!(active_files, 2, "expected two active snapshots persisted");

    let spectrum_files = std::fs::read_dir(spectrum_history_dir(&state_dir))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("snapshot-") && n.ends_with(".json"))
        })
        .count();
    assert_eq!(
        spectrum_files, 2,
        "expected two spectrum snapshots persisted"
    );
}

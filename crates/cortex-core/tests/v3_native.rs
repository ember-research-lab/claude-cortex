//! End-to-end test of the v3 native ledger:
//! create ledger → keypair → append blocks → record outcomes → verify chain.

use cortex_core::models::{Identity, LearningCategory, OutcomeResult};
use cortex_core::{Learning, Ledger};
use tempfile::TempDir;

#[test]
fn create_append_record_verify_roundtrip() {
    let dir = TempDir::new().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    let key_manager = ledger.key_manager();
    key_manager
        .generate_keypair(&Identity {
            name: "test".into(),
            machine: "ci".into(),
            email: None,
        })
        .unwrap();

    let learnings_a = vec![
        Learning::new(
            LearningCategory::Discovery,
            "v3 substrate writes RFC3339 Z timestamps",
            0.6,
            Some("v3-spec".into()),
        ),
        Learning::new(
            LearningCategory::Pattern,
            "atomic writes use temp + rename inside an exclusive flock",
            0.5,
            None,
        ),
    ];
    let block_a = ledger.append_block("session-1", learnings_a, true).unwrap();
    assert!(block_a.signature.is_some());

    let learnings_b = vec![Learning::new(
        LearningCategory::Decision,
        "block hash uses sorted-key serde_json with no whitespace",
        0.55,
        None,
    )];
    let block_b = ledger.append_block("session-2", learnings_b, true).unwrap();
    assert_eq!(block_b.parent_block.as_deref(), Some(block_a.id.as_str()));

    let confidence_after = ledger
        .record_outcome(&block_a.learnings[0].id, OutcomeResult::Success, "shipped")
        .unwrap();
    assert!((confidence_after - 0.7).abs() < 1e-9);

    let report = ledger.verify_chain().unwrap();
    assert!(report.is_clean(), "expected clean chain, got: {report:?}");
    assert_eq!(report.valid_blocks.len(), 2);

    let index = ledger.read_index().unwrap();
    assert_eq!(index.head.as_deref(), Some(block_b.id.as_str()));
    assert!(index.merkle_root.is_some());
}

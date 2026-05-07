//! v2 ledger compatibility: read the on-disk fixture under
//! `tests/fixtures/v2_ledger/`, recompute every block hash with the v2
//! Python-compatible canonical JSON, and verify the merkle root matches.

use cortex_core::merkle::MerkleTree;
use cortex_core::v2_compat::{
    compute_v2_block_hash, list_block_ids, load_block, load_index, verify_v2_block_hashes,
};
use std::path::PathBuf;

fn fixture_root() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("tests/fixtures/v2_ledger"))?;
    if root.is_dir() {
        Some(root)
    } else {
        None
    }
}

#[test]
fn v2_index_has_blocks_dir_with_matching_block_ids() {
    let Some(root) = fixture_root() else {
        eprintln!("skipping: no v2 fixture present");
        return;
    };
    let index = load_index(&root).unwrap();
    let listed = list_block_ids(&root).unwrap();
    let from_index: Vec<String> = index.blocks.iter().map(|b| b.id.clone()).collect();
    let mut from_index_sorted = from_index.clone();
    from_index_sorted.sort();
    assert_eq!(from_index_sorted, listed, "index/blocks dir mismatch");
    assert!(index.head.is_some());
}

#[test]
fn v2_block_hashes_recompute() {
    let Some(root) = fixture_root() else {
        eprintln!("skipping: no v2 fixture present");
        return;
    };
    let report = verify_v2_block_hashes(&root).unwrap();
    assert!(
        report.is_clean(),
        "v2 fixture verification dirty: {report:?}"
    );
    assert!(!report.valid_blocks.is_empty());
}

#[test]
fn v2_merkle_root_matches_recomputed_tree() {
    let Some(root) = fixture_root() else {
        eprintln!("skipping: no v2 fixture present");
        return;
    };
    let index = load_index(&root).unwrap();
    let leaves = index.blocks.iter().map(|b| (b.id.clone(), b.hash.clone()));
    let computed = MerkleTree::build(leaves);
    assert_eq!(
        computed.root_hash().map(str::to_string),
        index.merkle_root,
        "merkle root mismatch with v2 fixture"
    );
}

#[test]
fn v2_block_hash_recomputes_for_each_block() {
    let Some(root) = fixture_root() else {
        eprintln!("skipping: no v2 fixture present");
        return;
    };
    for block_id in list_block_ids(&root).unwrap() {
        let block = load_block(&root, &block_id).unwrap();
        let computed = compute_v2_block_hash(&block).unwrap();
        assert_eq!(
            computed, block.hash,
            "stored vs recomputed mismatch for block {block_id}"
        );
    }
}

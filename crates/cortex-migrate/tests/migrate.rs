//! End-to-end test of cortex-migrate: invoke the binary against the v2
//! fixture, verify the v3 output round-trips through `Ledger::verify_chain`.

use std::path::PathBuf;
use std::process::Command;

use cortex_core::Ledger;
use tempfile::TempDir;

fn fixture_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/v2_ledger")
}

fn binary() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/cortex-migrate")
}

#[test]
fn migrates_v2_fixture_into_v3_native_layout() {
    let v2 = fixture_root();
    if !v2.is_dir() {
        eprintln!("skipping: no v2 fixture present at {}", v2.display());
        return;
    }
    let dest = TempDir::new().unwrap();
    let bin = binary();
    if !bin.is_file() {
        eprintln!(
            "skipping: cortex-migrate binary not built (expected at {})",
            bin.display()
        );
        return;
    }

    let out = Command::new(&bin)
        .args([
            "--from",
            v2.to_str().unwrap(),
            "--to",
            dest.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cortex-migrate failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("v2 validation OK"));
    assert!(stderr.contains("migration complete"));

    // Audit file should be present.
    assert!(dest.path().join("MIGRATION.json").is_file());
    // v3 ledger should round-trip through verify_chain. Hashes recompute
    // because cortex-core uses canonical RFC3339 Z timestamps now.
    let ledger = Ledger::open(dest.path()).unwrap();
    let report = ledger.verify_chain().unwrap();
    // After transcribe we drop signatures (v2 sigs covered different bytes); accept if
    // the only failures are signature_failures from the dropped sigs, but in our migration
    // we set signature=None so this should be entirely clean.
    assert!(
        report.is_clean(),
        "expected clean v3 ledger after migration, got: {report:?}"
    );
    assert_eq!(report.valid_blocks.len(), 2);

    let index = ledger.read_index().unwrap();
    assert!(index.head.is_some());
    assert_eq!(index.blocks.len(), 2);
    assert!(index.merkle_root.is_some());

    let reinforcements = ledger.read_reinforcements().unwrap();
    assert_eq!(reinforcements.learnings.len(), 3);
    let outcome_count: u64 = reinforcements
        .learnings
        .values()
        .map(|r| r.outcome_count)
        .sum();
    assert_eq!(outcome_count, 2);
}

#[test]
fn check_only_does_not_write() {
    let v2 = fixture_root();
    if !v2.is_dir() {
        eprintln!("skipping: no v2 fixture present");
        return;
    }
    let dest = TempDir::new().unwrap();
    let bin = binary();
    if !bin.is_file() {
        eprintln!("skipping: cortex-migrate binary not built");
        return;
    }
    let out = Command::new(&bin)
        .args([
            "--from",
            v2.to_str().unwrap(),
            "--to",
            dest.path().to_str().unwrap(),
            "--check",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("v2 validation OK"));
    assert!(stderr.contains("check-only mode"));
    // Nothing should have been written into the temp dir.
    let read = std::fs::read_dir(dest.path()).unwrap();
    assert!(read.count() == 0, "expected dest to be empty in check mode");
}

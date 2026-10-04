//! Verify every block of a ledger directory: hashes, signatures, Merkle root.
//!
//! Run against a COPY of a live ledger (the ledger takes a lock):
//! `cargo run -p cortex-core --example verify_ledger -- <ledger-dir>`. Exits non-zero unless clean.

fn main() -> cortex_core::Result<()> {
    let root = std::env::args()
        .nth(1)
        .expect("usage: verify_ledger <ledger-dir>");
    let report = cortex_core::Ledger::open(&root)?.verify_chain()?;
    println!(
        "valid={} missing={} hash_mismatches={} signature_failures={} merkle_mismatch={}",
        report.valid_blocks.len(),
        report.missing_blocks.len(),
        report.hash_mismatches.len(),
        report.signature_failures.len(),
        report.merkle_mismatch.is_some()
    );
    for (id, check) in &report.signature_failures {
        println!("signature failure: {id} {check:?}");
    }
    if !report.is_clean() {
        std::process::exit(1);
    }
    Ok(())
}

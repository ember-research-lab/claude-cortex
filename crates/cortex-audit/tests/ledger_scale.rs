//! Ledger growth benchmark — what one business's action-audit ledger costs as it
//! grows under daily use (the per-business-per-server SMB-platform deployment).
//!
//! Not a correctness test; a measurement harness. `#[ignore]`d so CI compiles it on
//! every OS but never runs it. A **fixed signing seed** makes journals reproducible
//! across processes, so a file built once can be recover-measured by a later run.
//!
//! ```sh
//! # headline: build N + recover + measure (one size per process → clean peak RSS)
//! LEDGER_SCALE_N=1000000 cargo test --release -p cortex-audit --test ledger_scale \
//!     -- --ignored --nocapture ledger_growth_recovery_cost
//!
//! # build + KEEP the journal, then measure recovery ALONE on it (clean recover-phase
//! # peak — used to A/B the streaming-recovery change):
//! LEDGER_SCALE_N=1000000 LEDGER_SCALE_KEEP=1 cargo test --release -p cortex-audit \
//!     --test ledger_scale -- --ignored --nocapture ledger_growth_recovery_cost
//! LEDGER_SCALE_RECOVER=/tmp/cortex_ledger_scale_1000000/audit.jsonl \
//!     cargo test --release -p cortex-audit --test ledger_scale \
//!     -- --ignored --nocapture recover_only
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

use cortex_audit::{ActionRecord, AuditLedger};
use ed25519_dalek::SigningKey;

/// Fixed signing seed → reproducible journals across processes (so `recover_only`
/// can verify a journal an earlier `ledger_growth_recovery_cost --keep` built).
const SEED: [u8; 32] = [7u8; 32];
fn key() -> SigningKey {
    SigningKey::from_bytes(&SEED)
}

/// Peak resident set size (`VmHWM`) in KiB from `/proc` — Linux/WSL only; `None`
/// elsewhere (the harness still runs, just without the RAM figure).
fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|kib| kib.parse().ok())
    })
}

/// A realistic action record — the shape the SMB orchestrator dispatches: a workflow
/// action by a tenant agent, with a short human detail varied by `i`.
fn record(i: u64) -> ActionRecord {
    ActionRecord {
        principal: "agent:acme:leadgen".into(),
        tenant: "acme".into(),
        kind: "workflow.action".into(),
        detail: format!("save_lead id=l{i:08} segment=b2b-cleaning score=80"),
        timestamp_ms: 1_700_000_000_000 + i,
    }
}

fn report_recovery(path: &Path) {
    let rec_start = Instant::now();
    let led = AuditLedger::recover(path, key()).expect("recover");
    let rec_secs = rec_start.elapsed().as_secs_f64();
    let n = led.len(); // the FULL chain length (entries() is only the resident window)
    let resident = led.entries().len();
    let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    println!("\n=== recover @ N = {n} entries (resident window: {resident}) ===");
    println!(
        "disk:     {:8.1} MiB  ({:.0} bytes/entry)",
        file_bytes as f64 / (1024.0 * 1024.0),
        file_bytes as f64 / n.max(1) as f64
    );
    println!(
        "recover:  {rec_secs:8.2}s  ({:>10.0} entries/s)  ← paid on EVERY restart",
        n as f64 / rec_secs.max(1e-9)
    );
    match peak_rss_kib() {
        Some(kib) => {
            println!(
                "peak RSS: {:8.1} MiB  ({:.0} bytes/entry resident)",
                kib as f64 / 1024.0,
                (kib as f64 * 1024.0) / n.max(1) as f64
            );
        }
        None => println!("peak RSS: (unavailable — not Linux/proc)"),
    }
}

/// Headline: build N entries (real write-through append) then recover + measure.
/// `LEDGER_SCALE_KEEP=1` keeps the journal and prints its path for `recover_only`.
#[test]
#[ignore = "manual scale benchmark; run with LEDGER_SCALE_N and --nocapture"]
fn ledger_growth_recovery_cost() {
    let n: u64 = std::env::var("LEDGER_SCALE_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);
    let keep = std::env::var("LEDGER_SCALE_KEEP").is_ok();

    let dir = std::env::temp_dir().join(format!("cortex_ledger_scale_{n}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("audit.jsonl");

    let build_start = Instant::now();
    {
        let mut led = AuditLedger::recover(&path, key()).expect("open journal");
        for i in 0..n {
            led.append(record(i)).expect("append");
        }
    }
    let build_secs = build_start.elapsed().as_secs_f64();
    println!(
        "\nbuild:    {build_secs:8.2}s  ({:>10.0} appends/s)",
        n as f64 / build_secs.max(1e-9)
    );

    report_recovery(&path);

    if keep {
        println!("KEPT journal at: {}", path.display());
    } else {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Recover-only: measure recovery cost on a pre-built journal (clean recover-phase
/// peak RSS, no build in this process). Point `LEDGER_SCALE_RECOVER` at a journal a
/// prior `--keep` run produced. Used to A/B the recovery path.
#[test]
#[ignore = "manual; set LEDGER_SCALE_RECOVER=<journal path> and --nocapture"]
fn recover_only() {
    let path = std::env::var("LEDGER_SCALE_RECOVER")
        .map(PathBuf::from)
        .expect("set LEDGER_SCALE_RECOVER=<journal path>");
    report_recovery(&path);
}

//! cortex-dream — dreaming-pass binary (v4).
//!
//! Reads the cortex ledger, indexes its content via BM25 (cortex-similarity),
//! builds the learning graph, decomposes the Laplacian, emits an
//! active-memory snapshot, and records a spectrum-history snapshot for
//! cortex-monitor.
//!
//! Status: SCAFFOLD — only the CLI surface and the orchestrating function
//! signature are present. The actual pipeline is wired up in Phase 5.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "cortex-dream", version, about)]
struct Args {
    /// Path to the v3 ledger directory. Defaults to the global ledger
    /// (`$HOME/.claude/ledger`) if --ledger is not provided.
    #[arg(long)]
    ledger: Option<PathBuf>,

    /// Path to the cortex-state directory (active memory + spectrum history
    /// live here). Defaults to `<ledger>/cortex-state` so each ledger has
    /// its own state alongside its substrate.
    #[arg(long)]
    state: Option<PathBuf>,

    /// Override top-k eigenmode count. Default: `min(50, n / 3)` from
    /// cortex-spectral::default_top_k.
    #[arg(long)]
    k: Option<usize>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    eprintln!(
        "cortex-dream v{} — scaffold only (v4 Phase 5)",
        env!("CARGO_PKG_VERSION")
    );
    eprintln!(
        "  ledger: {:?}\n  state:  {:?}\n  k:      {:?}",
        args.ledger, args.state, args.k
    );
    eprintln!("(no work performed yet — pipeline lands in v4 Phase 5 implementation)");
    Ok(())
}

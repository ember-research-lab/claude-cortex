//! cortex-dream CLI entrypoint. The actual pipeline lives in `lib.rs` so
//! integration tests can call it directly.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "cortex-dream", version, about)]
struct Args {
    /// Path to the v3 ledger directory. Defaults to `$HOME/.claude/ledger`
    /// if --ledger is not provided.
    #[arg(long)]
    ledger: Option<PathBuf>,

    /// Path to the cortex-state directory (active memory + spectrum
    /// history live here). Defaults to `<ledger>/cortex-state` so each
    /// ledger has its own state alongside its substrate.
    #[arg(long)]
    state: Option<PathBuf>,

    /// Override top-k eigenmode count. Default: `min(50, n / 3)` from
    /// `cortex_spectral::default_top_k`.
    #[arg(long)]
    k: Option<usize>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let ledger_path = resolve_ledger_path(args.ledger.as_deref())?;
    let state_path = args
        .state
        .clone()
        .unwrap_or_else(|| ledger_path.join("cortex-state"));

    eprintln!("cortex-dream v{}", env!("CARGO_PKG_VERSION"));
    eprintln!("  ledger: {}", ledger_path.display());
    eprintln!("  state:  {}", state_path.display());

    let started = Instant::now();
    let report = cortex_dream::run(&ledger_path, &state_path, args.k)?;
    let elapsed = started.elapsed();

    eprintln!("  pipeline complete in {:.2}s", elapsed.as_secs_f64());
    eprintln!("  nodes:                 {}", report.n_nodes);
    eprintln!("  edges:                 {}", report.n_edges);
    eprintln!("  k (eigenmodes):        {}", report.k);
    eprintln!("  solver:                {:?}", report.solver);
    eprintln!("  active memory entries: {}", report.entries);
    eprintln!(
        "  active snapshot:       {}",
        report.active_snapshot.display()
    );
    eprintln!(
        "  spectrum snapshot:     {}",
        report.spectrum_snapshot.display()
    );
    Ok(())
}

fn resolve_ledger_path(explicit: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| anyhow::anyhow!("HOME not set; pass --ledger explicitly"))?;
    Ok(PathBuf::from(home).join(".claude/ledger"))
}

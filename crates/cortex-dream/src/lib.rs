//! cortex-dream library — the dreaming pipeline as a callable function.
//!
//! Pipeline:
//! 1. Read every learning from the v3 ledger via cortex-core.
//! 2. Build a BM25 index over learning content (cortex-similarity).
//! 3. Build the learning graph from pairwise BM25 similarity
//!    (cortex-spectral). Co-occurrence and outcome correlation are
//!    passed in as empty maps for v4.0 — those signals require separate
//!    instrumentation that lands in v4 minor releases.
//! 4. Compute the top-k eigendecomposition of the Laplacian.
//! 5. Build the active-memory snapshot and write it to
//!    `<state>/active/active-{ts}.json`, advancing `current`.
//! 6. Record a spectrum snapshot under
//!    `<state>/spectrum-history/snapshot-{ts}.json`.
//!
//! Performance budget per the v4 spec: <60s for ledgers under 10k
//! entries. Dense eigendecomposition path covers small ledgers (n < 500)
//! at O(n^3); above that, Lanczos integration is required (deferred to a
//! v4 minor release).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use cortex_core::Ledger;
use cortex_similarity::Bm25Index;
use cortex_spectral::{default_top_k, NodeId};
use uuid::Uuid;

#[derive(Debug)]
pub struct DreamReport {
    pub n_nodes: usize,
    pub n_edges: usize,
    pub k: usize,
    pub solver: cortex_spectral::SolverKind,
    pub entries: usize,
    pub active_snapshot: PathBuf,
    pub spectrum_snapshot: PathBuf,
}

pub fn run(
    ledger_path: &Path,
    state_path: &Path,
    k_override: Option<usize>,
) -> anyhow::Result<DreamReport> {
    let ledger = Ledger::open(ledger_path)
        .with_context(|| format!("opening ledger at {}", ledger_path.display()))?;
    let reinforcements = ledger.read_reinforcements()?;

    // Step 1 + 2: collect learnings, index BM25.
    let mut bm25 = Bm25Index::new();
    let mut learnings: Vec<(NodeId, String)> = Vec::new();
    for (learning_id, reinforcement) in reinforcements.learnings {
        bm25.add(reinforcement.content_hash.clone(), &reinforcement.content);
        learnings.push((NodeId(reinforcement.content_hash), learning_id));
    }
    bm25.recompute_stats();

    // Step 3: build graph.
    let similarity = bm25.pairwise_similarity();
    let co_occurrence: BTreeMap<(NodeId, NodeId), f64> = BTreeMap::new();
    let outcome_correlation: BTreeMap<(NodeId, NodeId), f64> = BTreeMap::new();
    let graph = cortex_spectral::build_graph(
        &learnings,
        &similarity,
        &co_occurrence,
        &outcome_correlation,
    );

    // Step 4: eigendecomposition.
    let k = k_override.unwrap_or_else(|| default_top_k(graph.n()));
    let eigendecomp = cortex_spectral::compute_eigendecomposition(&graph, k)?;

    // Step 5: active memory snapshot.
    let active = cortex_active_memory::build_active_memory(&graph, &eigendecomp, k)?;
    let active_snapshot = cortex_active_memory::write_snapshot(state_path, &active)?;

    // Step 6: spectrum snapshot for cortex-monitor.
    let spectrum = cortex_monitor::snapshot_from_eigendecomposition(
        &eigendecomp,
        active.timestamp.clone(),
        Uuid::new_v4().to_string(),
    );
    cortex_monitor::record_snapshot(state_path, &spectrum)?;
    let spectrum_snapshot = cortex_monitor::spectrum_history_dir(state_path).join(format!(
        "snapshot-{}.json",
        spectrum.timestamp.replace(':', "-")
    ));

    Ok(DreamReport {
        n_nodes: graph.n(),
        n_edges: graph.edges.len(),
        k,
        solver: eigendecomp.solver,
        entries: active.entries.len(),
        active_snapshot,
        spectrum_snapshot,
    })
}

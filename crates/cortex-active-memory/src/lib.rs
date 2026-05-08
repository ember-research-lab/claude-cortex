//! Active memory layer (v4) — curated working set built from the top-k
//! dominant eigenmodes of the learning-graph Laplacian.
//!
//! ## Persistence
//!
//! - `<state-root>/active/active-{rfc3339-z}.json` — immutable snapshot.
//! - `<state-root>/active/current` — pointer file holding the filename
//!   of the latest snapshot. Updated atomically via tempfile + rename so
//!   readers never observe a partial write. We use a pointer file rather
//!   than a symlink because Windows symlinks need elevated permissions
//!   and the pointer pattern is portable.
//! - Old snapshots accumulate on disk; cortex-monitor's spectrum history
//!   relies on this. Garbage collection deferred to a v4 minor release.

use std::fs;
use std::path::{Path, PathBuf};

use cortex_spectral::{Eigendecomposition, LearningGraph, NodeId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveMemory {
    pub snapshot_id: String,
    pub timestamp: String,
    /// Hashes of ledger blocks the snapshot was built from.
    #[serde(default)]
    pub source_block_hashes: Vec<String>,
    pub eigenmode_count: usize,
    /// Eigenvalues of the top-k modes that built this snapshot. Indexed
    /// matching `ActiveEntry::mode_projections`. Stored so cortex-mcp can
    /// do spectral retrieval at query time without recomputing the
    /// eigendecomposition.
    #[serde(default)]
    pub eigenvalues: Vec<f64>,
    pub entries: Vec<ActiveEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveEntry {
    pub node: NodeId,
    pub learning_id: String,
    /// Σ_i λ_i · v_i[node]² across the top-k eigenmodes — the node's
    /// projection magnitude into the dominant subspace. Higher = more
    /// active in the current spectral structure.
    pub projection_weight: f64,
    /// Per-eigenmode projection of this node: `mode_projections[i] = v_i[node]`.
    /// Length equals `ActiveMemory::eigenmode_count`. Lets cortex-mcp
    /// project a query into the same eigenmode basis at retrieval time.
    #[serde(default)]
    pub mode_projections: Vec<f64>,
}

pub fn active_memory_dir(state_root: &Path) -> PathBuf {
    state_root.join("active")
}

pub fn current_pointer_path(state_root: &Path) -> PathBuf {
    active_memory_dir(state_root).join("current")
}

fn snapshot_filename(snapshot: &ActiveMemory) -> String {
    let safe_ts = snapshot.timestamp.replace(':', "-");
    format!("active-{safe_ts}.json")
}

/// Build active memory from a graph + eigendecomposition + k.
///
/// Each node's projection_weight is `Σ_i λ_i · v_i[node]²` over the
/// modes in the eigendecomposition (this is the resonance score from
/// `cortex_spectral::resonance_score` evaluated on the standard basis
/// vector at that node). Entries are sorted descending by projection
/// weight; ties broken by node id for determinism. The output is
/// truncated to top-k entries.
pub fn build_active_memory(
    graph: &LearningGraph,
    eigendecomp: &Eigendecomposition,
    k: usize,
) -> anyhow::Result<ActiveMemory> {
    let n = graph.n();
    let timestamp = current_timestamp();
    let snapshot_id = Uuid::new_v4().to_string();

    let eigenvalues: Vec<f64> = eigendecomp.modes.iter().map(|m| m.eigenvalue).collect();

    if n == 0 || eigendecomp.modes.is_empty() {
        return Ok(ActiveMemory {
            snapshot_id,
            timestamp,
            source_block_hashes: Vec::new(),
            eigenmode_count: eigendecomp.modes.len(),
            eigenvalues,
            entries: Vec::new(),
        });
    }

    let mut entries: Vec<ActiveEntry> = (0..n)
        .map(|node_idx| {
            let mode_projections: Vec<f64> = eigendecomp
                .modes
                .iter()
                .map(|m| m.eigenvector.get(node_idx).copied().unwrap_or(0.0))
                .collect();
            let weight: f64 = eigendecomp
                .modes
                .iter()
                .zip(mode_projections.iter())
                .map(|(m, v)| m.eigenvalue * v * v)
                .sum();
            ActiveEntry {
                node: graph.nodes[node_idx].id.clone(),
                learning_id: graph.nodes[node_idx].learning_id.clone(),
                projection_weight: weight,
                mode_projections,
            }
        })
        .collect();

    entries.sort_by(|a, b| {
        b.projection_weight
            .partial_cmp(&a.projection_weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.node.0.cmp(&b.node.0))
    });
    entries.truncate(k);

    Ok(ActiveMemory {
        snapshot_id,
        timestamp,
        source_block_hashes: Vec::new(),
        eigenmode_count: eigendecomp.modes.len(),
        eigenvalues,
        entries,
    })
}

fn current_timestamp() -> String {
    // Path-safe RFC3339-Z form (colons replaced).
    let now = chrono::Utc::now();
    now.format("%Y-%m-%dT%H-%M-%S%.6fZ").to_string()
}

/// Write a snapshot to disk + atomically update `current` to point at it.
/// The snapshot file itself is written via a temp+rename pair, then the
/// `current` pointer is also updated via temp+rename, so concurrent
/// readers see either the old or new snapshot but never a torn state.
pub fn write_snapshot(state_root: &Path, snapshot: &ActiveMemory) -> anyhow::Result<PathBuf> {
    let dir = active_memory_dir(state_root);
    fs::create_dir_all(&dir)?;

    let filename = snapshot_filename(snapshot);
    let target = dir.join(&filename);
    let tmp_target = dir.join(format!("{filename}.{}.tmp", Uuid::new_v4().simple()));
    let bytes = serde_json::to_vec_pretty(snapshot)?;
    fs::write(&tmp_target, &bytes)?;
    if let Err(e) = fs::rename(&tmp_target, &target) {
        let _ = fs::remove_file(&tmp_target);
        return Err(e.into());
    }

    let current = current_pointer_path(state_root);
    let tmp_pointer = dir.join(format!("current.{}.tmp", Uuid::new_v4().simple()));
    fs::write(&tmp_pointer, filename.as_bytes())?;
    if let Err(e) = fs::rename(&tmp_pointer, &current) {
        let _ = fs::remove_file(&tmp_pointer);
        return Err(e.into());
    }

    Ok(target)
}

/// Normalized projection weight for a learning, in `[0, 1]`. Returns
/// `None` if the learning isn't in the snapshot or the snapshot has no
/// usable max weight.
///
/// Used by cortex-mcp to derive "spectral confidence": when active memory
/// exists, an entry's confidence becomes its share of the dominant
/// subspace's projection mass. The most-active entry has confidence 1.0;
/// less-active entries scale proportionally. Entries not in the snapshot
/// fall back to scalar v3 confidence at the call site.
pub fn spectral_confidence(snapshot: &ActiveMemory, learning_id: &str) -> Option<f64> {
    let max = snapshot
        .entries
        .iter()
        .map(|e| e.projection_weight)
        .fold(f64::NEG_INFINITY, f64::max);
    if !max.is_finite() || max <= 0.0 {
        return None;
    }
    snapshot
        .entries
        .iter()
        .find(|e| e.learning_id == learning_id)
        .map(|e| (e.projection_weight / max).clamp(0.0, 1.0))
}

/// Spectral retrieval: rank entries by alignment between the query's
/// node-space score vector and the eigenmode basis the snapshot
/// captures.
///
/// Given `query_scores` keyed by node id (typically BM25 score of the
/// query against each node's content), we project the query into each
/// eigenmode (`c_i = Σ_n query_scores[n] · v_i[n]`) and reconstruct
/// per-entry resonance: `score(e) = Σ_i λ_i · c_i · v_i[e]`. Entries are
/// returned sorted by descending resonance score, ties broken by
/// learning_id for determinism.
///
/// Returns an empty Vec if the snapshot has no eigenvalues or entries.
pub fn spectral_query<F: Fn(&NodeId) -> f64>(
    snapshot: &ActiveMemory,
    query_score: F,
) -> Vec<(&ActiveEntry, f64)> {
    if snapshot.entries.is_empty() || snapshot.eigenvalues.is_empty() {
        return Vec::new();
    }
    let k = snapshot.eigenvalues.len();
    // c_i = Σ_n query_score(n) · v_i[n]; n ranges over snapshot entries.
    let mut c = vec![0.0_f64; k];
    for entry in &snapshot.entries {
        let q = query_score(&entry.node);
        if q == 0.0 {
            continue;
        }
        for (i, ci) in c.iter_mut().enumerate() {
            let vi = entry.mode_projections.get(i).copied().unwrap_or(0.0);
            *ci += q * vi;
        }
    }
    let mut ranked: Vec<(&ActiveEntry, f64)> = snapshot
        .entries
        .iter()
        .map(|e| {
            let score: f64 = (0..k)
                .map(|i| {
                    let lambda = snapshot.eigenvalues[i];
                    let vi = e.mode_projections.get(i).copied().unwrap_or(0.0);
                    lambda * c[i] * vi
                })
                .sum();
            (e, score)
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.learning_id.cmp(&b.0.learning_id))
    });
    ranked
}

/// Read whichever snapshot `current` points at, or `None` if no current
/// pointer exists. Returns an error only on I/O / parse failure of an
/// existing pointer or snapshot.
pub fn read_current(state_root: &Path) -> anyhow::Result<Option<ActiveMemory>> {
    let pointer = current_pointer_path(state_root);
    if !pointer.is_file() {
        return Ok(None);
    }
    let filename = fs::read_to_string(&pointer)?.trim().to_string();
    if filename.is_empty() {
        return Ok(None);
    }
    let snapshot_path = active_memory_dir(state_root).join(&filename);
    if !snapshot_path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&snapshot_path)?;
    let snapshot: ActiveMemory = serde_json::from_slice(&bytes)?;
    Ok(Some(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_spectral::{Edge, EdgeWeightConfig, EdgeWeights, Node, SolverKind};
    use tempfile::TempDir;

    fn three_node_graph() -> LearningGraph {
        LearningGraph {
            nodes: vec![
                Node {
                    id: NodeId("a".into()),
                    learning_id: "la".into(),
                },
                Node {
                    id: NodeId("b".into()),
                    learning_id: "lb".into(),
                },
                Node {
                    id: NodeId("c".into()),
                    learning_id: "lc".into(),
                },
            ],
            edges: vec![
                Edge {
                    source: NodeId("a".into()),
                    target: NodeId("b".into()),
                    weights: EdgeWeights {
                        similarity: 1.0,
                        co_occurrence: 0.0,
                        outcome: 0.0,
                    },
                },
                Edge {
                    source: NodeId("b".into()),
                    target: NodeId("c".into()),
                    weights: EdgeWeights {
                        similarity: 1.0,
                        co_occurrence: 0.0,
                        outcome: 0.0,
                    },
                },
            ],
            edge_weight_config: EdgeWeightConfig {
                co_occurrence_weight: 0.0,
                similarity_weight: 1.0,
                outcome_weight: 0.0,
            },
        }
    }

    #[test]
    fn paths_are_stable() {
        let root = Path::new("/tmp/cortex-state");
        assert_eq!(
            active_memory_dir(root),
            PathBuf::from("/tmp/cortex-state/active")
        );
        assert_eq!(
            current_pointer_path(root),
            PathBuf::from("/tmp/cortex-state/active/current")
        );
    }

    #[test]
    fn empty_graph_yields_empty_active_memory() {
        let graph = LearningGraph::new();
        let eigen = Eigendecomposition {
            modes: Vec::new(),
            solver: SolverKind::Dense,
            n_nodes: 0,
        };
        let am = build_active_memory(&graph, &eigen, 5).unwrap();
        assert_eq!(am.entries.len(), 0);
    }

    #[test]
    fn entries_sorted_descending_by_projection_weight() {
        let graph = three_node_graph();
        let eigen = cortex_spectral::compute_eigendecomposition(&graph, 3).unwrap();
        let am = build_active_memory(&graph, &eigen, 3).unwrap();
        assert_eq!(am.entries.len(), 3);
        for w in am.entries.windows(2) {
            assert!(
                w[0].projection_weight >= w[1].projection_weight,
                "active entries not in descending order"
            );
        }
    }

    #[test]
    fn truncates_to_k_entries() {
        let graph = three_node_graph();
        let eigen = cortex_spectral::compute_eigendecomposition(&graph, 3).unwrap();
        let am = build_active_memory(&graph, &eigen, 1).unwrap();
        assert_eq!(am.entries.len(), 1);
    }

    #[test]
    fn projection_weight_matches_resonance_definition() {
        // For a 2-node unit-similarity graph, top eigenmode has λ=2 and
        // eigenvector ≈ [1/√2, -1/√2] (sign-canonicalized).
        // Projection weight per node = λ * v_node^2 = 2 * 0.5 = 1.0 each.
        // Plus the 0-eigenvalue mode contributes nothing.
        let graph = LearningGraph {
            nodes: vec![
                Node {
                    id: NodeId("a".into()),
                    learning_id: "la".into(),
                },
                Node {
                    id: NodeId("b".into()),
                    learning_id: "lb".into(),
                },
            ],
            edges: vec![Edge {
                source: NodeId("a".into()),
                target: NodeId("b".into()),
                weights: EdgeWeights {
                    similarity: 1.0,
                    co_occurrence: 0.0,
                    outcome: 0.0,
                },
            }],
            edge_weight_config: EdgeWeightConfig {
                co_occurrence_weight: 0.0,
                similarity_weight: 1.0,
                outcome_weight: 0.0,
            },
        };
        let eigen = cortex_spectral::compute_eigendecomposition(&graph, 2).unwrap();
        let am = build_active_memory(&graph, &eigen, 2).unwrap();
        assert_eq!(am.entries.len(), 2);
        for entry in &am.entries {
            assert!((entry.projection_weight - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn write_then_read_round_trips_active_memory() {
        let dir = TempDir::new().unwrap();
        let am = ActiveMemory {
            snapshot_id: "snap-1".into(),
            timestamp: "2026-05-07T22-15-00.000000Z".into(),
            source_block_hashes: vec!["block-abc".into()],
            eigenmode_count: 3,
            eigenvalues: vec![1.0, 0.5, 0.2],
            entries: vec![ActiveEntry {
                node: NodeId("a".into()),
                learning_id: "la".into(),
                projection_weight: 0.85,
                mode_projections: vec![0.7, 0.3, 0.1],
            }],
        };
        let path = write_snapshot(dir.path(), &am).unwrap();
        assert!(path.is_file());
        let loaded = read_current(dir.path()).unwrap().expect("no current");
        assert_eq!(loaded.snapshot_id, am.snapshot_id);
        assert_eq!(loaded.timestamp, am.timestamp);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].learning_id, "la");
        assert!((loaded.entries[0].projection_weight - 0.85).abs() < 1e-12);
        assert_eq!(loaded.entries[0].mode_projections.len(), 3);
        assert_eq!(loaded.eigenvalues.len(), 3);
    }

    #[test]
    fn write_advances_current_pointer() {
        let dir = TempDir::new().unwrap();
        let mk = |ts: &str, w: f64| ActiveMemory {
            snapshot_id: format!("snap-{ts}"),
            timestamp: ts.to_string(),
            source_block_hashes: Vec::new(),
            eigenmode_count: 1,
            eigenvalues: vec![1.0],
            entries: vec![ActiveEntry {
                node: NodeId("a".into()),
                learning_id: "la".into(),
                projection_weight: w,
                mode_projections: vec![1.0],
            }],
        };

        write_snapshot(dir.path(), &mk("2026-05-07T22-10-00.000000Z", 0.4)).unwrap();
        write_snapshot(dir.path(), &mk("2026-05-07T22-20-00.000000Z", 0.7)).unwrap();
        write_snapshot(dir.path(), &mk("2026-05-07T22-30-00.000000Z", 0.9)).unwrap();

        // Both old + new snapshot files should still exist.
        let active_dir = active_memory_dir(dir.path());
        let count = fs::read_dir(&active_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("active-") && n.ends_with(".json"))
            })
            .count();
        assert_eq!(count, 3);

        // current should point at the most recent.
        let current = read_current(dir.path()).unwrap().expect("no current");
        assert_eq!(current.timestamp, "2026-05-07T22-30-00.000000Z");
        assert!((current.entries[0].projection_weight - 0.9).abs() < 1e-12);
    }

    #[test]
    fn read_current_on_missing_state_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_current(dir.path()).unwrap().is_none());
    }

    fn snapshot_with(entries: Vec<(&str, f64, Vec<f64>)>, eigenvalues: Vec<f64>) -> ActiveMemory {
        ActiveMemory {
            snapshot_id: "test".into(),
            timestamp: "2026-05-07T00-00-00.000000Z".into(),
            source_block_hashes: Vec::new(),
            eigenmode_count: eigenvalues.len(),
            eigenvalues,
            entries: entries
                .into_iter()
                .map(|(lid, w, projs)| ActiveEntry {
                    node: NodeId(format!("n-{lid}")),
                    learning_id: lid.into(),
                    projection_weight: w,
                    mode_projections: projs,
                })
                .collect(),
        }
    }

    #[test]
    fn spectral_confidence_normalizes_to_one_for_max_entry() {
        let am = snapshot_with(
            vec![
                ("a", 1.0, vec![0.5]),
                ("b", 0.5, vec![0.3]),
                ("c", 0.25, vec![0.1]),
            ],
            vec![1.0],
        );
        assert_eq!(spectral_confidence(&am, "a"), Some(1.0));
        assert_eq!(spectral_confidence(&am, "b"), Some(0.5));
        assert_eq!(spectral_confidence(&am, "c"), Some(0.25));
    }

    #[test]
    fn spectral_confidence_returns_none_for_unknown_or_empty() {
        let empty = snapshot_with(Vec::new(), vec![1.0]);
        assert!(spectral_confidence(&empty, "missing").is_none());
        let am = snapshot_with(vec![("a", 0.5, vec![0.4])], vec![1.0]);
        assert!(spectral_confidence(&am, "missing").is_none());
    }

    #[test]
    fn spectral_query_ranks_aligned_node_first() {
        // Two-node, single-mode snapshot. eigenvector [1.0, -0.5].
        // Query scores node-a high, node-b low → projection c_1 ≈ 1.0
        // → resonance for entry-a = λ * c * v_a = 1*1.0*1.0 = 1.0
        // → resonance for entry-b = λ * c * v_b = 1*1.0*(-0.5) = -0.5
        // → entry-a should rank first.
        let am = snapshot_with(
            vec![("a", 0.0, vec![1.0]), ("b", 0.0, vec![-0.5])],
            vec![1.0],
        );
        let ranked = spectral_query(&am, |id| if id.0 == "n-a" { 1.0 } else { 0.0 });
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0.learning_id, "a");
        assert!(ranked[0].1 > ranked[1].1);
    }

    #[test]
    fn spectral_query_with_zero_query_yields_zero_scores() {
        let am = snapshot_with(
            vec![("a", 0.0, vec![1.0]), ("b", 0.0, vec![-1.0])],
            vec![1.0],
        );
        let ranked = spectral_query(&am, |_| 0.0);
        for (_, score) in &ranked {
            assert_eq!(*score, 0.0);
        }
    }

    #[test]
    fn spectral_query_returns_empty_for_empty_snapshot() {
        let empty = snapshot_with(Vec::new(), Vec::new());
        let ranked = spectral_query(&empty, |_| 1.0);
        assert!(ranked.is_empty());
    }
}

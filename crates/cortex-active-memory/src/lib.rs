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
    /// Hashes of ledger blocks the snapshot was built from. Lets cortex-monitor
    /// know which spectrum corresponds to which ledger range.
    #[serde(default)]
    pub source_block_hashes: Vec<String>,
    pub eigenmode_count: usize,
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

    if n == 0 || eigendecomp.modes.is_empty() {
        return Ok(ActiveMemory {
            snapshot_id,
            timestamp,
            source_block_hashes: Vec::new(),
            eigenmode_count: eigendecomp.modes.len(),
            entries: Vec::new(),
        });
    }

    let mut entries: Vec<ActiveEntry> = (0..n)
        .map(|node_idx| {
            let weight: f64 = eigendecomp
                .modes
                .iter()
                .map(|m| {
                    let v = m.eigenvector.get(node_idx).copied().unwrap_or(0.0);
                    m.eigenvalue * v * v
                })
                .sum();
            ActiveEntry {
                node: graph.nodes[node_idx].id.clone(),
                learning_id: graph.nodes[node_idx].learning_id.clone(),
                projection_weight: weight,
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
            entries: vec![ActiveEntry {
                node: NodeId("a".into()),
                learning_id: "la".into(),
                projection_weight: 0.85,
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
    }

    #[test]
    fn write_advances_current_pointer() {
        let dir = TempDir::new().unwrap();
        let mk = |ts: &str, w: f64| ActiveMemory {
            snapshot_id: format!("snap-{ts}"),
            timestamp: ts.to_string(),
            source_block_hashes: Vec::new(),
            eigenmode_count: 1,
            entries: vec![ActiveEntry {
                node: NodeId("a".into()),
                learning_id: "la".into(),
                projection_weight: w,
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
}

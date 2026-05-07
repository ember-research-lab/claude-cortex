//! Learning graph, sparse Laplacian, and eigendecomposition for v4
//! spectral retrieval.
//!
//! Status: SCAFFOLD — types and trait stubs only. Edge weight computation,
//! Laplacian construction, and the Lanczos solver are deferred.
//!
//! ## Conceptual model
//!
//! The cortex ledger holds learnings. v4 builds a graph over them where
//! edges encode three independent signals:
//!
//! 1. **Co-occurrence**: which learnings activated together in a session
//!    (lookups + applications via `search_learnings` / `get_learning`).
//! 2. **Semantic similarity**: cosine similarity between embeddings.
//! 3. **Outcome correlation**: do these learnings' `record_outcome` results
//!    move together over time?
//!
//! Total edge weight is `0.4·co_occurrence + 0.4·similarity + 0.2·outcome`
//! (spec default). The graph Laplacian L = D − A captures the
//! information-flow structure; its dominant eigenvectors are the modes that
//! cortex's `cortex-active-memory` crate distills into a working set.
//!
//! ## Defaults baked in (matching the v4 spec)
//!
//! - **Edge weight components**: 0.4 / 0.4 / 0.2
//! - **Solver**: Lanczos for graphs ≥ 500 nodes, full nalgebra
//!   eigendecomposition below that. (Aaron's existing Rust spectral libs
//!   provide the Lanczos kernel; integration deferred to implementation.)
//! - **Top-k eigenmode count**: `min(50, n / 3)` so small ledgers don't
//!   collapse into a single eigenvector cluster.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Reference to a ledger entry by its content hash. Stable across sessions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    /// Learning UUID from the cortex-core ledger; useful for joining back.
    pub learning_id: String,
}

/// Decomposition of a single edge's weight. Each component is in `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EdgeWeights {
    pub co_occurrence: f64,
    pub similarity: f64,
    pub outcome: f64,
}

impl EdgeWeights {
    pub fn total(&self, cfg: &EdgeWeightConfig) -> f64 {
        cfg.co_occurrence_weight * self.co_occurrence
            + cfg.similarity_weight * self.similarity
            + cfg.outcome_weight * self.outcome
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EdgeWeightConfig {
    pub co_occurrence_weight: f64,
    pub similarity_weight: f64,
    pub outcome_weight: f64,
}

impl Default for EdgeWeightConfig {
    fn default() -> Self {
        Self {
            co_occurrence_weight: 0.4,
            similarity_weight: 0.4,
            outcome_weight: 0.2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub source: NodeId,
    pub target: NodeId,
    pub weights: EdgeWeights,
}

/// Sparse adjacency representation of the learning graph. Edges are stored
/// keyed by `(source, target)` ordered tuples; weights are the precomputed
/// total. Both directions stored explicitly so the graph behaves as
/// undirected without duplicating logic.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearningGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub edge_weight_config: EdgeWeightConfig,
}

impl Serialize for EdgeWeightConfig {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = ser.serialize_struct("EdgeWeightConfig", 3)?;
        s.serialize_field("co_occurrence_weight", &self.co_occurrence_weight)?;
        s.serialize_field("similarity_weight", &self.similarity_weight)?;
        s.serialize_field("outcome_weight", &self.outcome_weight)?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for EdgeWeightConfig {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            co_occurrence_weight: f64,
            similarity_weight: f64,
            outcome_weight: f64,
        }
        let r = Raw::deserialize(de)?;
        Ok(Self {
            co_occurrence_weight: r.co_occurrence_weight,
            similarity_weight: r.similarity_weight,
            outcome_weight: r.outcome_weight,
        })
    }
}

impl LearningGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn n(&self) -> usize {
        self.nodes.len()
    }

    pub fn node_index(&self, id: &NodeId) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == *id)
    }
}

/// One eigenmode: an eigenvalue and its corresponding unit eigenvector
/// indexed by node position in `LearningGraph::nodes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Eigenmode {
    pub eigenvalue: f64,
    pub eigenvector: Vec<f64>,
}

/// Top-k eigendecomposition of the graph Laplacian.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Eigendecomposition {
    pub modes: Vec<Eigenmode>,
    /// Order: by descending eigenvalue (dominant first).
    pub solver: SolverKind,
    pub n_nodes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolverKind {
    /// Full eigendecomposition — used when n is small enough that O(n^3)
    /// is fine (currently n < 500).
    Dense,
    /// Iterative Krylov subspace method — used for n ≥ 500.
    Lanczos,
}

/// Resonance score: weighted projection magnitude of a query vector onto
/// the top-k eigenmodes of the learning graph. Higher = more aligned with
/// the dominant subspace cortex has built.
pub fn resonance_score(query: &[f64], modes: &[Eigenmode]) -> f64 {
    let mut total = 0.0;
    for m in modes {
        let proj: f64 = query
            .iter()
            .zip(m.eigenvector.iter())
            .map(|(q, e)| q * e)
            .sum();
        total += m.eigenvalue * proj * proj;
    }
    total
}

/// Top-k recommendation per the spec: `min(50, n / 3)`.
pub fn default_top_k(n_nodes: usize) -> usize {
    let cap = 50;
    let scaled = (n_nodes / 3).max(1);
    cap.min(scaled)
}

/// **STUB** — build a graph from ledger learnings + their embeddings + outcome
/// history. Returns an empty graph with default config until Phase 2
/// implementation lands.
pub fn build_graph(
    _learnings: &[(NodeId, String)], // (id, learning_id)
    _embeddings: &BTreeMap<NodeId, Vec<f32>>,
    _co_occurrence: &BTreeMap<(NodeId, NodeId), f64>,
    _outcome_correlation: &BTreeMap<(NodeId, NodeId), f64>,
) -> LearningGraph {
    LearningGraph::new()
}

/// **STUB** — compute the top-k eigendecomposition of the graph Laplacian.
/// Returns empty until Phase 2 implementation lands. Selects solver per
/// `default_top_k`/`n` thresholds.
pub fn compute_eigendecomposition(
    graph: &LearningGraph,
    k: usize,
) -> anyhow::Result<Eigendecomposition> {
    Ok(Eigendecomposition {
        modes: Vec::new(),
        solver: if graph.n() < 500 {
            SolverKind::Dense
        } else {
            SolverKind::Lanczos
        },
        n_nodes: graph.n(),
    })
    .map(|mut d| {
        d.modes.truncate(k);
        d
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_edge_weights_sum_to_one() {
        let cfg = EdgeWeightConfig::default();
        let total = cfg.co_occurrence_weight + cfg.similarity_weight + cfg.outcome_weight;
        assert!((total - 1.0).abs() < 1e-12);
    }

    #[test]
    fn default_top_k_handles_small_ledgers() {
        assert_eq!(default_top_k(0), 1); // floor at 1
        assert_eq!(default_top_k(3), 1);
        assert_eq!(default_top_k(30), 10);
        assert_eq!(default_top_k(150), 50); // capped
        assert_eq!(default_top_k(10_000), 50); // still capped
    }

    #[test]
    fn resonance_with_zero_query_is_zero() {
        let modes = vec![Eigenmode {
            eigenvalue: 1.0,
            eigenvector: vec![1.0, 0.0],
        }];
        assert_eq!(resonance_score(&[0.0, 0.0], &modes), 0.0);
    }
}

//! Learning graph, sparse Laplacian, and eigendecomposition for v4
//! spectral retrieval.
//!
//! Status: SCAFFOLD — types only. Edge weight computation, Laplacian
//! construction, and the Lanczos solver are deferred to Phase 2 impl.
//!
//! Edge weight = `0.4·co_occurrence + 0.4·similarity + 0.2·outcome` (spec
//! default). Top-k = `min(50, n / 3)` to scale gracefully on small ledgers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub learning_id: String,
}

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearningGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub edge_weight_config: EdgeWeightConfig,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Eigenmode {
    pub eigenvalue: f64,
    pub eigenvector: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Eigendecomposition {
    pub modes: Vec<Eigenmode>,
    pub solver: SolverKind,
    pub n_nodes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolverKind {
    Dense,
    Lanczos,
}

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

pub fn default_top_k(n_nodes: usize) -> usize {
    let cap = 50;
    let scaled = (n_nodes / 3).max(1);
    cap.min(scaled)
}

/// Build a learning graph from ledger learnings + their pairwise BM25
/// similarity + co-occurrence + outcome correlation. **STUB** — Phase 2
/// fills in the real construction; for now returns an empty graph.
///
/// `similarity` is the matrix from
/// `cortex_similarity::Bm25Index::pairwise_similarity()` — values
/// already normalized to `[0, 1]`.
pub fn build_graph(
    _learnings: &[(NodeId, String)],
    _similarity: &cortex_similarity::SimilarityMatrix,
    _co_occurrence: &BTreeMap<(NodeId, NodeId), f64>,
    _outcome_correlation: &BTreeMap<(NodeId, NodeId), f64>,
) -> LearningGraph {
    LearningGraph::new()
}

pub fn compute_eigendecomposition(
    graph: &LearningGraph,
    k: usize,
) -> anyhow::Result<Eigendecomposition> {
    let solver = if graph.n() < 500 {
        SolverKind::Dense
    } else {
        SolverKind::Lanczos
    };
    let mut decomp = Eigendecomposition {
        modes: Vec::new(),
        solver,
        n_nodes: graph.n(),
    };
    decomp.modes.truncate(k);
    Ok(decomp)
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
        assert_eq!(default_top_k(0), 1);
        assert_eq!(default_top_k(3), 1);
        assert_eq!(default_top_k(30), 10);
        assert_eq!(default_top_k(150), 50);
        assert_eq!(default_top_k(10_000), 50);
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

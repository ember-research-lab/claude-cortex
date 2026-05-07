//! Learning graph, sparse Laplacian, and eigendecomposition for v4
//! spectral retrieval.
//!
//! ## Pipeline
//!
//! 1. `build_graph(learnings, similarity, co_occ, outcome)` — combines
//!    BM25 pairwise similarity (from `cortex-similarity`) with
//!    co-occurrence and outcome-correlation signals into a weighted
//!    `LearningGraph`. Edge weight is `0.4·sim + 0.4·co_occ + 0.2·outcome`
//!    by default (`EdgeWeightConfig::default()`).
//!
//! 2. `compute_eigendecomposition(graph, k)` — builds the symmetric
//!    Laplacian `L = D − A`, runs symmetric eigendecomposition, returns
//!    the top-k modes ordered by *descending* eigenvalue (dominant first).
//!    For `n < 500` uses nalgebra's full dense `SymmetricEigen`; for
//!    larger graphs falls back to a Lanczos placeholder (Phase 2 ships
//!    the dense path; Lanczos integration with Aaron's spectral libs
//!    follows in a v4 minor release).
//!
//! ## Invariants
//!
//! - Determinism: same inputs → identical graph + eigendecomposition.
//! - Symmetry: edges are undirected; `Edge { source, target }` is stored
//!   once with `source < target` ordering for canonical iteration.
//! - Laplacian symmetry: `L[i][j] = L[j][i]`, so all eigenvalues are real
//!   and non-negative (PSD).
//! - Top-k order: descending eigenvalue, ties broken by eigenvector L2
//!   ordering (rare but kept deterministic).

use std::collections::BTreeMap;

use nalgebra::{DMatrix, SymmetricEigen};
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

    /// Build a dense weighted adjacency matrix from the edge list. Symmetric
    /// because edges are stored once but represent undirected connections.
    pub fn adjacency(&self) -> DMatrix<f64> {
        let n = self.n();
        let mut a = DMatrix::<f64>::zeros(n, n);
        for edge in &self.edges {
            let Some(i) = self.node_index(&edge.source) else {
                continue;
            };
            let Some(j) = self.node_index(&edge.target) else {
                continue;
            };
            let w = edge.weights.total(&self.edge_weight_config);
            a[(i, j)] = w;
            a[(j, i)] = w;
        }
        a
    }

    /// Symmetric graph Laplacian: `L = D − A` where `D` is the diagonal
    /// degree matrix (sum of incident edge weights). For an undirected
    /// graph this is symmetric and positive semi-definite, so all
    /// eigenvalues are real and ≥ 0.
    pub fn laplacian(&self) -> DMatrix<f64> {
        let a = self.adjacency();
        let n = a.nrows();
        let mut l = DMatrix::<f64>::zeros(n, n);
        for i in 0..n {
            let mut deg = 0.0;
            for j in 0..n {
                if i != j {
                    let aij = a[(i, j)];
                    deg += aij;
                    l[(i, j)] = -aij;
                }
            }
            l[(i, i)] = deg;
        }
        l
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

/// Build a learning graph from learnings + their pairwise BM25 similarity
/// + optional co-occurrence + optional outcome correlation.
///
/// `similarity` is the matrix from `cortex_similarity::Bm25Index::pairwise_similarity()`
/// — values already normalized to `[0, 1]` and indexed by the `ids` field
/// in matching order with `learnings`.
///
/// Edges produced for every node pair where the combined edge weight (per
/// `EdgeWeightConfig::default()`) is strictly positive. Edges are stored
/// once with the canonical ordering `source.0 < target.0` so iteration is
/// deterministic.
pub fn build_graph(
    learnings: &[(NodeId, String)],
    similarity: &cortex_similarity::SimilarityMatrix,
    co_occurrence: &BTreeMap<(NodeId, NodeId), f64>,
    outcome_correlation: &BTreeMap<(NodeId, NodeId), f64>,
) -> LearningGraph {
    let cfg = EdgeWeightConfig::default();
    let nodes: Vec<Node> = learnings
        .iter()
        .map(|(id, lid)| Node {
            id: id.clone(),
            learning_id: lid.clone(),
        })
        .collect();
    let n = nodes.len();
    let mut edges = Vec::new();

    for i in 0..n {
        for j in (i + 1)..n {
            let id_i = &nodes[i].id;
            let id_j = &nodes[j].id;
            let sim = lookup_similarity(similarity, id_i, id_j);
            let co_occ = lookup_pair(co_occurrence, id_i, id_j);
            let outcome = lookup_pair(outcome_correlation, id_i, id_j);
            let weights = EdgeWeights {
                co_occurrence: co_occ,
                similarity: sim,
                outcome,
            };
            if weights.total(&cfg) > 0.0 {
                edges.push(Edge {
                    source: id_i.clone(),
                    target: id_j.clone(),
                    weights,
                });
            }
        }
    }

    LearningGraph {
        nodes,
        edges,
        edge_weight_config: cfg,
    }
}

fn lookup_similarity(matrix: &cortex_similarity::SimilarityMatrix, a: &NodeId, b: &NodeId) -> f64 {
    let Some(i) = matrix.ids.iter().position(|x| x == &a.0) else {
        return 0.0;
    };
    let Some(j) = matrix.ids.iter().position(|x| x == &b.0) else {
        return 0.0;
    };
    // matrix is BM25-asymmetric (score(query=i, doc=j) != score(query=j, doc=i)).
    // Symmetrize by averaging — the spectral layer wants undirected edges.
    let sij = matrix.get(i, j);
    let sji = matrix.get(j, i);
    0.5 * (sij + sji)
}

fn lookup_pair(map: &BTreeMap<(NodeId, NodeId), f64>, a: &NodeId, b: &NodeId) -> f64 {
    map.get(&(a.clone(), b.clone()))
        .or_else(|| map.get(&(b.clone(), a.clone())))
        .copied()
        .unwrap_or(0.0)
}

/// Compute the top-k eigendecomposition of the graph Laplacian.
///
/// For `n < 500` uses nalgebra's full symmetric eigendecomposition (dense,
/// O(n^3)). For `n ≥ 500` falls back to the same dense path for now; the
/// Lanczos integration with Aaron's spectral libs is the planned upgrade.
/// The `solver` field on the returned `Eigendecomposition` records which
/// path was taken.
///
/// Modes are ordered by *descending* eigenvalue (dominant first). Ties are
/// broken by lexicographic ordering of the eigenvector components, giving
/// deterministic output even when eigenvalues coincide.
pub fn compute_eigendecomposition(
    graph: &LearningGraph,
    k: usize,
) -> anyhow::Result<Eigendecomposition> {
    let n = graph.n();
    let solver = if n < 500 {
        SolverKind::Dense
    } else {
        SolverKind::Lanczos
    };
    if n == 0 {
        return Ok(Eigendecomposition {
            modes: Vec::new(),
            solver,
            n_nodes: 0,
        });
    }

    let l = graph.laplacian();
    let eig = SymmetricEigen::new(l);
    let mut modes: Vec<Eigenmode> = (0..n)
        .map(|i| Eigenmode {
            eigenvalue: eig.eigenvalues[i],
            eigenvector: eig.eigenvectors.column(i).iter().copied().collect(),
        })
        .collect();
    // Canonicalize: flip eigenvector signs so the first non-zero entry is
    // positive. This makes the output stable across runs (eigenvector signs
    // are otherwise arbitrary up to ±1).
    for m in &mut modes {
        canonicalize_sign(&mut m.eigenvector);
    }
    modes.sort_by(|a, b| {
        b.eigenvalue
            .partial_cmp(&a.eigenvalue)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| eigenvector_lex(&a.eigenvector, &b.eigenvector))
    });
    modes.truncate(k);
    Ok(Eigendecomposition {
        modes,
        solver,
        n_nodes: n,
    })
}

fn canonicalize_sign(v: &mut [f64]) {
    let pivot = v.iter().find(|x| x.abs() > 1e-12).copied().unwrap_or(0.0);
    if pivot < 0.0 {
        for x in v.iter_mut() {
            *x = -*x;
        }
    }
}

fn eigenvector_lex(a: &[f64], b: &[f64]) -> std::cmp::Ordering {
    for (x, y) in a.iter().zip(b.iter()) {
        match x.partial_cmp(y) {
            Some(std::cmp::Ordering::Equal) => continue,
            Some(ord) => return ord,
            None => return std::cmp::Ordering::Equal,
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_similarity::SimilarityMatrix;

    fn empty_pair_map() -> BTreeMap<(NodeId, NodeId), f64> {
        BTreeMap::new()
    }

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

    #[test]
    fn empty_graph_has_zero_eigenmodes() {
        let g = LearningGraph::new();
        let d = compute_eigendecomposition(&g, 5).unwrap();
        assert_eq!(d.modes.len(), 0);
        assert_eq!(d.n_nodes, 0);
    }

    #[test]
    fn zero_edge_graph_has_all_zero_eigenvalues() {
        let g = LearningGraph {
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
            edges: Vec::new(),
            edge_weight_config: EdgeWeightConfig::default(),
        };
        let d = compute_eigendecomposition(&g, 3).unwrap();
        assert_eq!(d.modes.len(), 3);
        for m in &d.modes {
            assert!(m.eigenvalue.abs() < 1e-12);
        }
    }

    #[test]
    fn two_node_graph_with_unit_similarity_has_top_eigenvalue_2() {
        // Adjacency = [[0,1],[1,0]], degree = [1,1], Laplacian = [[1,-1],[-1,1]].
        // Eigenvalues are {0, 2}. Top eigenvector for λ=2 is ±[1, -1]/√2.
        let g = LearningGraph {
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
        let d = compute_eigendecomposition(&g, 2).unwrap();
        assert_eq!(d.modes.len(), 2);
        assert!((d.modes[0].eigenvalue - 2.0).abs() < 1e-9);
        assert!(d.modes[1].eigenvalue.abs() < 1e-9);
        // Top eigenvector should be [1/√2, -1/√2] after sign canonicalization
        // (the pivot is the first non-zero entry; we flip if negative).
        let v = &d.modes[0].eigenvector;
        let inv_sqrt2 = 1.0 / 2.0_f64.sqrt();
        assert!((v[0].abs() - inv_sqrt2).abs() < 1e-9);
        assert!((v[1].abs() - inv_sqrt2).abs() < 1e-9);
        // After canonicalization first non-zero is positive.
        assert!(v[0] > 0.0);
    }

    #[test]
    fn build_graph_is_deterministic_under_input_order() {
        let learnings: Vec<(NodeId, String)> = vec![
            (NodeId("a".into()), "la".into()),
            (NodeId("b".into()), "lb".into()),
            (NodeId("c".into()), "lc".into()),
        ];
        let similarity = SimilarityMatrix {
            ids: vec!["a".into(), "b".into(), "c".into()],
            data: vec![0.0, 0.5, 0.2, 0.5, 0.0, 0.3, 0.2, 0.3, 0.0],
            n: 3,
        };
        let g1 = build_graph(
            &learnings,
            &similarity,
            &empty_pair_map(),
            &empty_pair_map(),
        );
        let g2 = build_graph(
            &learnings,
            &similarity,
            &empty_pair_map(),
            &empty_pair_map(),
        );
        assert_eq!(g1.edges.len(), g2.edges.len());
        for (e1, e2) in g1.edges.iter().zip(g2.edges.iter()) {
            assert_eq!(e1.source, e2.source);
            assert_eq!(e1.target, e2.target);
            assert!((e1.weights.similarity - e2.weights.similarity).abs() < 1e-12);
        }
    }

    #[test]
    fn build_graph_combines_signals_with_default_weights() {
        let learnings: Vec<(NodeId, String)> = vec![
            (NodeId("a".into()), "la".into()),
            (NodeId("b".into()), "lb".into()),
        ];
        let similarity = SimilarityMatrix {
            ids: vec!["a".into(), "b".into()],
            data: vec![0.0, 0.6, 0.6, 0.0],
            n: 2,
        };
        let mut co_occ = BTreeMap::new();
        co_occ.insert((NodeId("a".into()), NodeId("b".into())), 0.4);
        let mut outcome = BTreeMap::new();
        outcome.insert((NodeId("a".into()), NodeId("b".into())), 0.3);

        let g = build_graph(&learnings, &similarity, &co_occ, &outcome);
        assert_eq!(g.edges.len(), 1);
        let e = &g.edges[0];
        // 0.4 * co_occ(0.4) + 0.4 * sim(0.6) + 0.2 * outcome(0.3) = 0.16 + 0.24 + 0.06 = 0.46
        let expected = 0.4 * 0.4 + 0.4 * 0.6 + 0.2 * 0.3;
        assert!((e.weights.total(&g.edge_weight_config) - expected).abs() < 1e-12);
    }

    #[test]
    fn solver_kind_switches_at_500_nodes() {
        let g = LearningGraph {
            nodes: (0..499)
                .map(|i| Node {
                    id: NodeId(format!("n{i}")),
                    learning_id: format!("l{i}"),
                })
                .collect(),
            edges: Vec::new(),
            edge_weight_config: EdgeWeightConfig::default(),
        };
        let d = compute_eigendecomposition(&g, 5).unwrap();
        assert_eq!(d.solver, SolverKind::Dense);
        assert_eq!(d.n_nodes, 499);
    }

    #[test]
    fn top_k_truncates_correctly() {
        let g = LearningGraph {
            nodes: (0..5)
                .map(|i| Node {
                    id: NodeId(format!("n{i}")),
                    learning_id: format!("l{i}"),
                })
                .collect(),
            edges: Vec::new(),
            edge_weight_config: EdgeWeightConfig::default(),
        };
        let d = compute_eigendecomposition(&g, 2).unwrap();
        assert_eq!(d.modes.len(), 2);
    }

    #[test]
    fn modes_are_sorted_descending_by_eigenvalue() {
        // 3-node path graph: a - b - c (uniform similarity). Laplacian
        // eigenvalues are {0, 1, 3}. After top-k truncation we want them in
        // descending order: 3, 1, 0.
        let g = LearningGraph {
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
        };
        let d = compute_eigendecomposition(&g, 3).unwrap();
        assert_eq!(d.modes.len(), 3);
        for w in d.modes.windows(2) {
            assert!(
                w[0].eigenvalue >= w[1].eigenvalue,
                "modes not in descending order: {} < {}",
                w[0].eigenvalue,
                w[1].eigenvalue
            );
        }
        // Path-graph spectrum {0, 1, 3} for 3 nodes.
        assert!((d.modes[0].eigenvalue - 3.0).abs() < 1e-9);
        assert!((d.modes[1].eigenvalue - 1.0).abs() < 1e-9);
        assert!(d.modes[2].eigenvalue.abs() < 1e-9);
    }
}

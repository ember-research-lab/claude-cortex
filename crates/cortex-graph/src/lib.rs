//! cortex-graph — project cortex learnings into an `ember-graph` knowledge graph
//! and answer questions with a token-budgeted, relationship-aware subgraph.
//!
//! This is the v-next retrieval substrate. Learnings become nodes (label = the
//! learning content, so [`ember_graph::Graph::query`] seeds on it); edges link
//! lexically-similar learnings (BM25, via `cortex-similarity`) so a query can
//! traverse to *related* learnings, not just lexically-matching ones.
//!
//! v1 edges are similarity-derived, so today's win over raw BM25 is the
//! **budget-bounded neighbourhood render + relevance scoping**. Richer edges
//! (co-occurrence from capture, corroboration from the confidence model, and
//! code-graph links) are fed in by later layers; this crate only owns the
//! projection + the budgeted query.

#[cfg(feature = "code-extract")]
pub mod code;

use cortex_similarity::Bm25Index;
use ember_graph::{Confidence, Edge, Graph, InferredTier, Node, NodeId};

/// A learning to project into the graph. Deliberately decoupled from
/// cortex-core's concrete block types — callers map their records in.
#[derive(Clone, Debug)]
pub struct LearningNode {
    pub id: String,
    pub content: String,
    pub category: String,
}

/// Namespace for learning-node ids (`learning::<id>`).
const NS_LEARNING: &str = "learning";

/// Minimum BM25 score for a similarity edge — below this, "related" is noise.
const EDGE_MIN_SCORE: f64 = 1.0;

/// Build an `ember-graph` [`Graph`] from a set of learnings. Each learning is a
/// node whose label is its content; each is linked to its top-`edge_top_k` most
/// BM25-similar peers (above [`EDGE_MIN_SCORE`]) so a query can traverse to
/// related learnings.
pub fn build_graph(learnings: &[LearningNode], edge_top_k: usize) -> Graph {
    let mut g = Graph::new();
    for l in learnings {
        g.add_node(Node {
            id: NodeId::canonical(NS_LEARNING, &l.id),
            label: l.content.clone(),
            kind: l.category.clone(),
            source: None,
        });
    }

    let mut bm25 = Bm25Index::new();
    for l in learnings {
        bm25.add(l.id.clone(), &l.content);
    }
    bm25.recompute_stats();

    for l in learnings {
        // top_k against this learning's own content; skip self, drop noise.
        for (peer_id, score) in bm25.top_k(&l.content, edge_top_k + 1) {
            if peer_id == l.id || score < EDGE_MIN_SCORE {
                continue;
            }
            g.add_edge(Edge {
                source: NodeId::canonical(NS_LEARNING, &l.id),
                target: NodeId::canonical(NS_LEARNING, &peer_id),
                relation: "similar_to".to_string(),
                confidence: Confidence::Inferred(InferredTier::Reasonable),
            });
        }
    }
    g
}

/// Answer `question` with a compact, budget-bounded subgraph of related
/// learnings — the retrieval primitive session-start / `search_learnings` feed on.
pub fn query(graph: &Graph, question: &str, depth: usize, budget_chars: usize) -> String {
    graph.query(question, depth, budget_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<LearningNode> {
        vec![
            LearningNode {
                id: "a".into(),
                content: "confidence decay relaxes toward the prior on disuse".into(),
                category: "pattern".into(),
            },
            LearningNode {
                id: "b".into(),
                content: "corroboration counts directly toward confidence".into(),
                category: "pattern".into(),
            },
            LearningNode {
                id: "c".into(),
                content: "WAL bloat corrupts the whale signal sqlite database".into(),
                category: "error".into(),
            },
        ]
    }

    #[test]
    fn projects_and_queries_within_budget() {
        let g = build_graph(&sample(), 3);
        let budget = 2000;
        let out = query(&g, "confidence decay prior", 2, budget);

        assert!(!out.is_empty(), "query returned nothing");
        assert!(out.len() <= budget, "result {} exceeded budget", out.len());
        assert!(
            out.contains("confidence"),
            "relevant learning missing:\n{out}"
        );
        // The unrelated WAL learning shares no terms and no edge — must be scoped out.
        assert!(
            !out.contains("whale signal"),
            "irrelevant learning leaked:\n{out}"
        );
    }

    #[test]
    fn similar_learnings_are_linked() {
        // 'a' and 'b' both concern confidence → expect a similarity edge a→b.
        let g = build_graph(&sample(), 3);
        let a = NodeId::canonical(NS_LEARNING, "a");
        let b = NodeId::canonical(NS_LEARNING, "b");
        let linked = g.neighbors(&a).iter().any(|e| e.target == b);
        assert!(linked, "'a' should link to 'b' (both about confidence)");
    }
}

//! BM25 lexical similarity for the cortex spectral layer (v4).
//!
//! This crate replaces the originally-planned `cortex-embeddings` crate
//! (which would have called the Anthropic API) with a fully self-contained
//! BM25 implementation. No external API, no model download, no auth, no
//! recurring cost — users get the v4 spectral layer working with nothing
//! beyond their Claude Code subscription.
//!
//! ## Why BM25
//!
//! BM25 is the dominant retrieval scoring function (the same one v2 cortex
//! used via SQLite FTS5). For short ledger entries (≤500 chars) lexical
//! overlap is a strong signal; semantic embeddings would help on longer or
//! paraphrased text but are unnecessary at the scales cortex operates on.
//!
//! Mathematically the spectral layer doesn't need vectors per se — it
//! needs a **pairwise similarity** signal to weight graph edges. BM25
//! gives us exactly that. For retrieval, the query becomes a "vector" in
//! node-similarity space (its BM25 score against each known node), and
//! that vector projects onto the eigenmodes the same way an embedding
//! would. See `docs/v4-plan-of-record.md`.
//!
//! ## Hyperparameters
//!
//! - `k1 = 1.5` — term saturation (standard).
//! - `b = 0.75` — length normalization (standard).
//!
//! ## Tokenization
//!
//! Lowercase, split on Unicode word boundaries, drop tokens shorter than
//! 2 chars. Deliberately simple — no stemming, no stopword list — because
//! cortex ledger content is short and vocabulary-dense, so aggressive
//! normalization loses signal.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub const BM25_K1: f64 = 1.5;
pub const BM25_B: f64 = 0.75;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub tokens: Vec<String>,
    pub length: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Bm25Index {
    pub documents: Vec<Document>,
    pub document_frequency: HashMap<String, u64>,
    pub avg_doc_length: f64,
    pub k1: f64,
    pub b: f64,
}

pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2)
        .map(|t| t.to_string())
        .collect()
}

impl Bm25Index {
    pub fn new() -> Self {
        Self {
            documents: Vec::new(),
            document_frequency: HashMap::new(),
            avg_doc_length: 0.0,
            k1: BM25_K1,
            b: BM25_B,
        }
    }

    pub fn from_corpus<I: IntoIterator<Item = (String, String)>>(items: I) -> Self {
        let mut index = Self::new();
        for (id, content) in items {
            index.add(id, &content);
        }
        index.recompute_stats();
        index
    }

    pub fn add(&mut self, id: impl Into<String>, content: &str) {
        let tokens = tokenize(content);
        let length = tokens.len();
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for t in &tokens {
            if seen.insert(t.as_str()) {
                *self.document_frequency.entry(t.clone()).or_insert(0) += 1;
            }
        }
        self.documents.push(Document {
            id: id.into(),
            tokens,
            length,
        });
    }

    pub fn recompute_stats(&mut self) {
        let n = self.documents.len();
        if n == 0 {
            self.avg_doc_length = 0.0;
            return;
        }
        let total: usize = self.documents.iter().map(|d| d.length).sum();
        self.avg_doc_length = total as f64 / n as f64;
    }

    fn idf(&self, term: &str) -> f64 {
        let n = self.documents.len() as f64;
        let nt = *self.document_frequency.get(term).unwrap_or(&0) as f64;
        ((n - nt + 0.5) / (nt + 0.5) + 1.0).ln()
    }

    pub fn score_doc(&self, query_tokens: &[String], doc: &Document) -> f64 {
        if doc.length == 0 || self.avg_doc_length <= 0.0 {
            return 0.0;
        }
        let mut score = 0.0;
        let mut tf: HashMap<&str, u64> = HashMap::new();
        for t in &doc.tokens {
            *tf.entry(t.as_str()).or_insert(0) += 1;
        }
        for q in query_tokens {
            let f = *tf.get(q.as_str()).unwrap_or(&0) as f64;
            if f == 0.0 {
                continue;
            }
            let idf = self.idf(q);
            let length_norm = 1.0 - self.b + self.b * (doc.length as f64 / self.avg_doc_length);
            let denom = f + self.k1 * length_norm;
            score += idf * (f * (self.k1 + 1.0)) / denom;
        }
        score
    }

    pub fn score_query(&self, query: &str) -> Vec<(String, f64)> {
        let q = tokenize(query);
        self.documents
            .iter()
            .map(|d| (d.id.clone(), self.score_doc(&q, d)))
            .collect()
    }

    pub fn top_k(&self, query: &str, k: usize) -> Vec<(String, f64)> {
        let mut scored = self.score_query(query);
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(k);
        scored
    }

    /// Pairwise similarity matrix for spectral graph construction.
    /// Each entry is the BM25 score of doc_j tokens against doc_i,
    /// normalized to `[0, 1]` by dividing by the max in the matrix.
    pub fn pairwise_similarity(&self) -> SimilarityMatrix {
        let n = self.documents.len();
        let mut data = vec![0.0_f64; n * n];
        let mut max = 0.0_f64;
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                let q = &self.documents[i].tokens;
                let s = self.score_doc(q, &self.documents[j]);
                data[i * n + j] = s;
                if s > max {
                    max = s;
                }
            }
        }
        if max > 0.0 {
            for v in &mut data {
                *v /= max;
            }
        }
        SimilarityMatrix {
            ids: self.documents.iter().map(|d| d.id.clone()).collect(),
            data,
            n,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarityMatrix {
    pub ids: Vec<String>,
    pub data: Vec<f64>,
    pub n: usize,
}

impl SimilarityMatrix {
    pub fn get(&self, i: usize, j: usize) -> f64 {
        self.data[i * self.n + j]
    }

    pub fn upper_triangle(&self) -> impl Iterator<Item = (usize, usize, f64)> + '_ {
        (0..self.n)
            .flat_map(move |i| ((i + 1)..self.n).map(move |j| (i, j, self.data[i * self.n + j])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_drops_short_tokens_and_punctuation() {
        let toks = tokenize("Hello, World! a b cd.");
        assert_eq!(toks, vec!["hello", "world", "cd"]);
    }

    #[test]
    fn empty_corpus_scores_zero() {
        let idx = Bm25Index::new();
        let scored = idx.score_query("anything");
        assert!(scored.is_empty());
    }

    #[test]
    fn identical_corpus_yields_nonzero_similarity() {
        let idx = Bm25Index::from_corpus(vec![
            ("a".into(), "atomic writes use tempfile rename".into()),
            ("b".into(), "atomic writes use tempfile rename".into()),
        ]);
        let m = idx.pairwise_similarity();
        assert!(m.get(0, 1) > 0.0);
        assert!(m.get(1, 0) > 0.0);
    }

    #[test]
    fn top_k_ranks_relevant_first() {
        let idx = Bm25Index::from_corpus(vec![
            (
                "auth".into(),
                "validate jwt token signature with public key".into(),
            ),
            (
                "db".into(),
                "create database migration script for users table".into(),
            ),
            (
                "auth2".into(),
                "rotate jwt signing keys without breaking existing sessions".into(),
            ),
        ]);
        let top = idx.top_k("jwt token", 2);
        let ids: Vec<&str> = top.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"auth"));
        assert!(!ids.contains(&"db"));
    }

    #[test]
    fn unrelated_documents_have_zero_pairwise_similarity() {
        let idx = Bm25Index::from_corpus(vec![
            (
                "a".into(),
                "cortex preserves substrate format byte for byte".into(),
            ),
            (
                "b".into(),
                "fantasy football roster construction superflex value".into(),
            ),
        ]);
        let m = idx.pairwise_similarity();
        assert_eq!(m.get(0, 1), 0.0);
    }

    #[test]
    fn idf_weights_rare_terms_higher() {
        let idx = Bm25Index::from_corpus(vec![
            ("a".into(), "common word here".into()),
            ("b".into(), "common word there".into()),
            ("c".into(), "rare unique terminology".into()),
        ]);
        assert!(idx.idf("rare") > idx.idf("common"));
    }
}

//! A/B: recall_context (graph) vs search_learnings (BM25/spectral) against the
//! REAL global ledger. Measures the char payload each hands the model (a token
//! proxy) and prints both so relevance can be eyeballed.
//!
//! Run: cargo run -p cortex-mcp --example ab_retrieval -- "your question here"

use cortex_mcp::tools::{args::*, impls};
use cortex_mcp::CortexServer;

#[tokio::main]
async fn main() {
    // No default project dir -> resolve_ledger falls back to the GLOBAL ledger.
    let server = CortexServer::new();
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "grok delegation confidence".to_string());

    let search = impls::search_learnings(
        &server,
        SearchLearningsArgs {
            query: query.clone(),
            category: None,
            min_confidence: 0.0,
            limit: 8,
            project_dir: None,
        },
    )
    .await
    .unwrap();

    let recall = impls::recall_context(
        &server,
        RecallContextArgs {
            question: query.clone(),
            budget_chars: Some(2000),
            depth: Some(2),
            project_dir: None,
        },
    )
    .await
    .unwrap();

    let empty = vec![];
    let results = search["results"].as_array().unwrap_or(&empty);
    let search_payload = serde_json::to_string(&search["results"]).unwrap().len();
    let search_content: usize = results
        .iter()
        .filter_map(|r| r.get("content").and_then(|c| c.as_str()))
        .map(|s| s.len())
        .sum();
    let ctx = recall["context"].as_str().unwrap_or("");

    println!("QUERY: {query}\n");
    println!("--- search_learnings ---");
    println!(
        "results: {}  | full JSON payload: {} chars | content-only: {} chars",
        results.len(),
        search_payload,
        search_content
    );
    for r in results.iter().take(8) {
        let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let conf = r.get("effective_confidence").and_then(|v| v.as_f64());
        let snip: String = r
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .chars()
            .take(70)
            .collect();
        println!("  [{id} {conf:?}] {snip}");
    }

    println!("\n--- recall_context (graph, budget 2000) ---");
    println!("context: {} chars", ctx.len());
    println!("{}", ctx);
}

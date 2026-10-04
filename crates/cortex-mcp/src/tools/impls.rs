//! Tool implementations. Each tool returns a JSON string. v2 returned `dict`
//! from Python; the rmcp Rust handler returns a `String` so we serialize the
//! response value into JSON ourselves to keep the wire format identical.

use std::path::PathBuf;

use anyhow::anyhow;
use chrono::Utc;
use cortex_core::confidence::effective_confidence_epistemic;
use cortex_core::models::{
    Block, Learning, LearningCategory, OutcomeResult, Reinforcement, Reinforcements,
};
use cortex_core::Ledger;
use rmcp::model::{CallToolResult, Content};
use rmcp::ErrorData;
use serde_json::{json, Map, Value};

use super::args::*;
use crate::paths::{global_ledger_path, project_ledger_path};
use crate::server::CortexServer;

/// Convert an `anyhow::Result<Value>` into an MCP tool result. The JSON body is
/// always kept for the model. Application errors (a failed call, or a payload
/// carrying a non-null `"error"`) set `isError=true` so the host and the model
/// can tell failure from success.
pub async fn run(
    future: impl std::future::Future<Output = anyhow::Result<Value>>,
) -> Result<CallToolResult, ErrorData> {
    let (value, is_error) = match future.await {
        Ok(v) => {
            let failed = v.get("error").is_some_and(|e| !e.is_null());
            (v, failed)
        }
        Err(e) => (json!({ "error": e.to_string() }), true),
    };
    let text = serde_json::to_string(&value)
        .map_err(|e| ErrorData::internal_error(format!("serialize tool result: {e}"), None))?;
    let content = vec![Content::text(text)];
    Ok(if is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    })
}

fn global_ledger(server: &CortexServer) -> Option<PathBuf> {
    server
        .global_ledger_override
        .clone()
        .or_else(global_ledger_path)
}

fn resolve_ledger(
    server: &CortexServer,
    project_dir: Option<&str>,
) -> anyhow::Result<Option<PathBuf>> {
    if let Some(p) = project_dir {
        return Ok(Some(project_ledger_path(Some(std::path::Path::new(p)))?));
    }
    if let Some(default) = &server.default_project_dir {
        return Ok(Some(project_ledger_path(Some(default.as_path()))?));
    }
    Ok(global_ledger(server))
}

/// Ledgers a READ tool should consult, in order, each labelled for the response.
/// An explicit `project_dir` (or the server default) means exactly that project
/// ledger. With neither, the project ledger of the cwd comes first and the
/// global ledger second, so recall from a project session sees project learnings
/// (B4). Only ledgers that exist on disk are returned. If NONE exists this is an
/// error naming every path looked at (surfaced as `isError`), never an empty
/// success.
fn read_candidates(
    server: &CortexServer,
    project_dir: Option<&str>,
) -> anyhow::Result<Vec<(&'static str, PathBuf)>> {
    let explicit = project_dir
        .map(PathBuf::from)
        .or_else(|| server.default_project_dir.clone());
    let mut wanted: Vec<(&'static str, PathBuf)> = Vec::new();
    if let Some(dir) = explicit {
        wanted.push(("project", project_ledger_path(Some(dir.as_path()))?));
    } else {
        let cwd = match &server.cwd_override {
            Some(c) => c.clone(),
            None => std::env::current_dir()?,
        };
        wanted.push(("project", project_ledger_path(Some(cwd.as_path()))?));
        if let Some(g) = global_ledger(server) {
            if !wanted.iter().any(|(_, p)| *p == g) {
                wanted.push(("global", g));
            }
        }
    }
    let found: Vec<(&'static str, PathBuf)> =
        wanted.iter().filter(|(_, p)| p.is_dir()).cloned().collect();
    if found.is_empty() {
        let looked: Vec<String> = wanted
            .iter()
            .map(|(l, p)| format!("{l} ledger {}", p.display()))
            .collect();
        return Err(anyhow!(
            "ledger_missing: no cortex ledger found (looked for {})",
            looked.join("; ")
        ));
    }
    Ok(found)
}

/// Run `one` against each candidate ledger in order and return the first result
/// for which `answered` holds, annotated with `"ledger"` (which one answered)
/// and `"ledgers_searched"`. If none answered, the first result is returned
/// with `"ledger": null`.
fn across(
    candidates: &[(&'static str, PathBuf)],
    mut one: impl FnMut(&std::path::Path) -> anyhow::Result<Value>,
    answered: impl Fn(&Value) -> bool,
) -> anyhow::Result<Value> {
    let searched = labels(candidates);
    let mut first: Option<Value> = None;
    for (label, path) in candidates {
        let mut result = one(path)?;
        result["ledger"] = json!(label);
        result["ledgers_searched"] = json!(searched);
        if answered(&result) {
            return Ok(result);
        }
        first.get_or_insert(result);
    }
    let mut none = first.expect("read_candidates never returns empty");
    none["ledger"] = Value::Null;
    Ok(none)
}

fn labels(candidates: &[(&'static str, PathBuf)]) -> Vec<&'static str> {
    candidates.iter().map(|(l, _)| *l).collect()
}

fn open_ledger(path: &std::path::Path) -> anyhow::Result<Option<Ledger>> {
    if !path.is_dir() {
        return Ok(None);
    }
    Ok(Some(Ledger::open(path)?))
}

fn parse_category(s: &str) -> anyhow::Result<LearningCategory> {
    match s.to_ascii_lowercase().as_str() {
        "discovery" => Ok(LearningCategory::Discovery),
        "decision" => Ok(LearningCategory::Decision),
        "error" => Ok(LearningCategory::Error),
        "pattern" => Ok(LearningCategory::Pattern),
        other => Err(anyhow!(
            "Category must be one of: discovery, decision, error, pattern (got {other})"
        )),
    }
}

fn category_value(c: LearningCategory) -> &'static str {
    match c {
        LearningCategory::Discovery => "discovery",
        LearningCategory::Decision => "decision",
        LearningCategory::Error => "error",
        LearningCategory::Pattern => "pattern",
    }
}

fn parse_outcome_result(s: &str) -> anyhow::Result<OutcomeResult> {
    match s.to_ascii_lowercase().as_str() {
        "success" => Ok(OutcomeResult::Success),
        "partial" => Ok(OutcomeResult::Partial),
        "failure" => Ok(OutcomeResult::Failure),
        other => Err(anyhow!(
            "Result must be one of: success, partial, failure (got {other})"
        )),
    }
}

fn round2(f: f64) -> f64 {
    (f * 100.0).round() / 100.0
}

fn effective_confidence(reinforcement: &Reinforcement) -> f64 {
    effective_confidence_epistemic(
        reinforcement.confidence,
        reinforcement.origin,
        reinforcement.last_applied.into_inner(),
        Utc::now(),
    )
}

/// Effective confidence with spectral fallback: if `active` is present
/// and the learning is in its top-k entries, use `spectral_confidence`
/// (normalized projection_weight). Otherwise fall back to scalar v3
/// confidence with 180-day decay. v4's substrate-inviolability principle
/// applied: scalar values are still recorded on writes; spectral values
/// only override at READ time when an active-memory snapshot exists.
fn confidence_with_spectral(
    reinforcement: &Reinforcement,
    learning_id: &str,
    active: Option<&cortex_active_memory::ActiveMemory>,
) -> f64 {
    if let Some(snapshot) = active {
        if let Some(spectral) = cortex_active_memory::spectral_confidence(snapshot, learning_id) {
            return spectral;
        }
    }
    effective_confidence(reinforcement)
}

/// Resolve the active-memory snapshot for a given ledger path, if one
/// exists. State directory is `<ledger>/cortex-state/` (matches what
/// cortex-dream writes).
fn load_active_memory(ledger_path: &std::path::Path) -> Option<cortex_active_memory::ActiveMemory> {
    let state = ledger_path.join("cortex-state");
    cortex_active_memory::read_current(&state).ok().flatten()
}

fn shortid(id: &str) -> String {
    id.chars().take(8).collect()
}

fn match_prefix<'a>(
    reinforcements: &'a Reinforcements,
    learning_id: &str,
) -> Option<(&'a String, &'a Reinforcement)> {
    if let Some(r) = reinforcements.learnings.get(learning_id) {
        return reinforcements
            .learnings
            .get_key_value(learning_id)
            .map(|(k, _)| (k, r));
    }
    let mut iter = reinforcements
        .learnings
        .iter()
        .filter(|(id, _)| id.starts_with(learning_id));
    let first = iter.next()?;
    if iter.next().is_some() {
        // Ambiguous prefix; refuse to guess.
        return None;
    }
    Some(first)
}

fn block_for_reinforcement(ledger: &Ledger, reinforcement: &Reinforcement) -> Option<Block> {
    ledger.read_block(&reinforcement.block_id).ok().flatten()
}

fn ledger_with_reinforcements(
    server: &CortexServer,
    project_dir: Option<&str>,
) -> anyhow::Result<Option<(Ledger, Reinforcements)>> {
    let Some(path) = resolve_ledger(server, project_dir)? else {
        return Ok(None);
    };
    let Some(ledger) = open_ledger(&path)? else {
        return Ok(None);
    };
    let reinforcements = ledger.read_reinforcements()?;
    Ok(Some((ledger, reinforcements)))
}

// ===== ledger-grounded tools =====

pub async fn recall_context(
    server: &CortexServer,
    args: RecallContextArgs,
) -> anyhow::Result<Value> {
    let budget = args.budget_chars.unwrap_or(2000);
    // Default depth 0 (BM25-seed render, NO traversal). Measured: cortex-graph's
    // BM25-SIMILARITY edges are noise even at 1 hop — a query's top BM25 neighbors
    // are often topically-tangential. So today recall_context == budget-bounded
    // BM25. The graph seam stays: bump depth once REAL edges exist (code links
    // from Phase 2b, corroboration links) — those, not similarity, justify
    // traversal. Callers can override `depth`.
    let depth = args.depth.unwrap_or(0);

    let candidates = read_candidates(server, args.project_dir.as_deref())?;
    across(
        &candidates,
        |path| recall_one(path, &args.question, depth, budget),
        |r| r["context"].as_str().is_some_and(|c| !c.is_empty()),
    )
}

fn recall_one(
    path: &std::path::Path,
    question: &str,
    depth: usize,
    budget: usize,
) -> anyhow::Result<Value> {
    let reinforcements = Ledger::open(path)?.read_reinforcements()?;

    let nodes: Vec<cortex_graph::LearningNode> = reinforcements
        .learnings
        .iter()
        .map(|(id, r)| cortex_graph::LearningNode {
            id: id.clone(),
            content: r.content.clone(),
            category: format!("{:?}", r.category),
        })
        .collect();

    // edge_top_k = 0: build NO similarity edges. Measured that BM25-similarity
    // edges are noise — `render` prints a node's outgoing edges, so any edge
    // shows as a "similar_to" line regardless of traversal depth. With no edges,
    // recall_context is an honest budget-bounded BM25 seed render. (This makes
    // cortex-graph == cortex-similarity + render TODAY; the graph earns its keep
    // only when REAL edges — code links, corroboration — are added.)
    let g = cortex_graph::build_graph(&nodes, 0);
    let text = cortex_graph::query(&g, question, depth, budget);
    Ok(json!({"context": text, "budget": budget, "error": null}))
}

/// Number of index blocks the snapshot was NOT built from. A snapshot built
/// from `source_block_hashes` only covers the ledger while every current block
/// hash is listed; legacy snapshots (empty list) therefore count as stale
/// (fail-closed).
fn uncovered_blocks(snapshot: &cortex_active_memory::ActiveMemory, ledger: &Ledger) -> usize {
    let Ok(index) = ledger.read_index() else {
        return usize::MAX;
    };
    let covered: std::collections::HashSet<&str> = snapshot
        .source_block_hashes
        .iter()
        .map(String::as_str)
        .collect();
    index
        .blocks
        .iter()
        .filter(|b| !covered.contains(b.hash.as_str()))
        .count()
}

pub async fn search_learnings(
    server: &CortexServer,
    args: SearchLearningsArgs,
) -> anyhow::Result<Value> {
    let category_filter = match args.category.as_deref() {
        Some(c) => Some(parse_category(c)?),
        None => None,
    };
    let candidates = read_candidates(server, args.project_dir.as_deref())?;
    across(
        &candidates,
        |path| search_one(path, &args, category_filter),
        |r| r["total"].as_u64().unwrap_or(0) > 0,
    )
}

fn search_one(
    path: &std::path::Path,
    args: &SearchLearningsArgs,
    category_filter: Option<LearningCategory>,
) -> anyhow::Result<Value> {
    let ledger = Ledger::open(path)?;
    // A corrupt/unreadable snapshot is reported (`snapshot_error`), not swallowed.
    let (active, snapshot_error) =
        match cortex_active_memory::read_current(&path.join("cortex-state")) {
            Ok(a) => (a, None),
            Err(e) => (None, Some(e.to_string())),
        };
    let reinforcements = ledger.read_reinforcements()?;
    let has_query = !args.query.trim().is_empty();

    // Spectral retrieval only when the snapshot covers the current index head;
    // a stale snapshot would rank a frozen subset and hide newer learnings (B1).
    let mut snapshot_stale: Option<usize> = None;
    let spectral = match &active {
        Some(snapshot) if has_query => match uncovered_blocks(snapshot, &ledger) {
            0 => Some(snapshot),
            n => {
                snapshot_stale = Some(n);
                None
            }
        },
        _ => None,
    };

    let mut bm25 = cortex_similarity::Bm25Index::new();
    for r in reinforcements.learnings.values() {
        bm25.add(r.content_hash.clone(), &r.content);
    }
    bm25.recompute_stats();
    let query_scores: std::collections::HashMap<String, f64> =
        bm25.score_query(&args.query).into_iter().collect();

    let mut scored: Vec<(String, Reinforcement, f64, f64)> = Vec::new();
    let mode = if let Some(snapshot) = spectral {
        let ranked = cortex_active_memory::spectral_query(snapshot, |node_id| {
            query_scores.get(&node_id.0).copied().unwrap_or(0.0)
        });
        for (entry, resonance) in ranked {
            // Zero-score hits are noise, never results.
            if resonance <= 0.0 {
                continue;
            }
            let Some((id, r)) = reinforcements
                .learnings
                .iter()
                .find(|(id, _)| id.as_str() == entry.learning_id)
            else {
                continue;
            };
            if category_filter.is_some_and(|f| r.category != f) {
                continue;
            }
            if cortex_core::confidence::is_contested(r.origin) {
                continue; // Contested facts are quarantined from retrieval.
            }
            let conf = confidence_with_spectral(r, id, Some(snapshot));
            if conf < args.min_confidence {
                continue;
            }
            scored.push((id.clone(), r.clone(), conf, resonance));
        }
        "spectral"
    } else {
        // BM25, not substring: substring returned nothing for natural-language
        // queries (measured 3/3).
        for (id, r) in &reinforcements.learnings {
            if category_filter.is_some_and(|f| r.category != f) {
                continue;
            }
            if cortex_core::confidence::is_contested(r.origin) {
                continue; // Contested facts are quarantined from retrieval.
            }
            let relevance = query_scores.get(&r.content_hash).copied().unwrap_or(0.0);
            // A non-empty query must lexically match (BM25 > 0); an empty query
            // lists everything (ordered by confidence).
            if has_query && relevance <= 0.0 {
                continue;
            }
            let conf = effective_confidence(r);
            if conf < args.min_confidence {
                continue;
            }
            scored.push((id.clone(), r.clone(), conf, relevance));
        }
        scored.sort_by(|a, b| {
            b.3.partial_cmp(&a.3)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
        });
        "bm25"
    };
    scored.truncate(args.limit);
    let results: Vec<Value> = scored
        .iter()
        .enumerate()
        .map(|(rank, (id, r, conf, resonance))| {
            let snippet: String = r.content.chars().take(150).collect();
            let mut entry = json!({
                "id": shortid(id),
                "full_id": id,
                "snippet": snippet,
                "category": category_value(r.category),
                "confidence": round2(*conf),
                "rank": rank,
            });
            if mode == "spectral" {
                entry["resonance"] = json!(round2(*resonance));
            }
            entry
        })
        .collect();
    let mut out = json!({
        "query": args.query,
        "category": args.category,
        "mode": mode,
        "results": results,
        "total": results.len(),
    });
    if let Some(n) = snapshot_stale {
        out["snapshot_stale"] = json!(true);
        out["uncovered"] = json!(n);
    }
    if let Some(reason) = snapshot_error {
        out["snapshot_error"] = json!(reason);
    }
    Ok(out)
}

pub async fn get_learning(server: &CortexServer, args: GetLearningArgs) -> anyhow::Result<Value> {
    let candidates = read_candidates(server, args.project_dir.as_deref())?;
    let mut found = None;
    for (label, path) in &candidates {
        let ledger = Ledger::open(path)?;
        let reinforcements = ledger.read_reinforcements()?;
        if match_prefix(&reinforcements, &args.learning_id).is_some() {
            found = Some((*label, path.clone(), ledger, reinforcements));
            break;
        }
    }
    let Some((label, path, ledger, reinforcements)) = found else {
        return Ok(json!({
            "error": format!(
                "Learning '{}' not found (ledgers searched: {})",
                args.learning_id,
                labels(&candidates).join(", ")
            ),
            "ledgers_searched": labels(&candidates),
        }));
    };
    let active = load_active_memory(&path);
    let (id, reinforcement) =
        match_prefix(&reinforcements, &args.learning_id).expect("matched above");
    let block = block_for_reinforcement(&ledger, reinforcement);
    let mut result = Map::new();
    result.insert("ledger".into(), Value::String(label.to_string()));
    result.insert("id".into(), Value::String(id.clone()));
    result.insert(
        "category".into(),
        Value::String(category_value(reinforcement.category).into()),
    );
    result.insert(
        "content".into(),
        Value::String(reinforcement.content.clone()),
    );
    result.insert("confidence".into(), json!(round2(reinforcement.confidence)));
    let source = block
        .as_ref()
        .and_then(|b| b.learnings.iter().find(|l| l.id == *id))
        .and_then(|l| l.source.clone());
    result.insert("source".into(), Value::from(source));
    let created = block
        .as_ref()
        .map(|b| b.timestamp.as_str())
        .map(Value::String)
        .unwrap_or(Value::Null);
    result.insert("created".into(), created);
    if args.show_decay {
        let effective = confidence_with_spectral(reinforcement, id, active.as_ref());
        result.insert("effective_confidence".into(), json!(round2(effective)));
        let mode = if active.is_some()
            && cortex_active_memory::spectral_confidence(active.as_ref().unwrap(), id).is_some()
        {
            "spectral"
        } else {
            "scalar"
        };
        result.insert("confidence_mode".into(), Value::String(mode.to_string()));
        result.insert(
            "has_decayed".into(),
            Value::Bool(effective < reinforcement.confidence),
        );
    }
    if args.show_outcomes {
        let outcomes: Vec<Value> = reinforcement
            .outcomes
            .iter()
            .map(|o| {
                json!({
                    "timestamp": o.timestamp.as_str(),
                    "result": match o.result {
                        OutcomeResult::Success => "success",
                        OutcomeResult::Failure => "failure",
                        OutcomeResult::Partial => "partial",
                    },
                    "context": o.context,
                    "delta": o.delta,
                })
            })
            .collect();
        result.insert("outcomes".into(), Value::Array(outcomes));
    }
    Ok(Value::Object(result))
}

pub async fn record_outcome(
    server: &CortexServer,
    args: RecordOutcomeArgs,
) -> anyhow::Result<Value> {
    let outcome = parse_outcome_result(&args.result)?;
    let Some((ledger, reinforcements)) =
        ledger_with_reinforcements(server, args.project_dir.as_deref())?
    else {
        return Ok(json!({"error": "Ledger not found"}));
    };
    let Some((id, _)) = match_prefix(&reinforcements, &args.learning_id) else {
        return Ok(json!({
            "error": format!("Learning '{}' not found", args.learning_id)
        }));
    };
    let id = id.clone();
    let context = args.comment.unwrap_or_default();
    let new_confidence = ledger.record_outcome(&id, outcome, context)?;
    Ok(json!({
        "status": "recorded",
        "learning_id": shortid(&id),
        "result": args.result.to_lowercase(),
        "new_confidence": round2(new_confidence),
    }))
}

pub async fn record_corroboration(
    server: &CortexServer,
    args: RecordCorroborationArgs,
) -> anyhow::Result<Value> {
    let Some((ledger, reinforcements)) =
        ledger_with_reinforcements(server, args.project_dir.as_deref())?
    else {
        return Ok(json!({"error": "Ledger not found"}));
    };
    let Some((id, _)) = match_prefix(&reinforcements, &args.learning_id) else {
        return Ok(json!({
            "error": format!("Learning '{}' not found", args.learning_id)
        }));
    };
    let id = id.clone();
    let context = args.context.unwrap_or_default();
    let (corroboration, confidence) = ledger.record_corroboration(&id, context)?;
    Ok(json!({
        "learning_id": args.learning_id,
        "corroboration": corroboration,
        "confidence": confidence,
        "error": null,
    }))
}

pub async fn list_learnings(
    server: &CortexServer,
    args: ListLearningsArgs,
) -> anyhow::Result<Value> {
    let category_filter = match args.category.as_deref() {
        Some(c) => Some(parse_category(c)?),
        None => None,
    };
    let candidates = read_candidates(server, args.project_dir.as_deref())?;
    across(
        &candidates,
        |path| list_one(path, &args, category_filter),
        |r| r["total"].as_u64().unwrap_or(0) > 0,
    )
}

fn list_one(
    path: &std::path::Path,
    args: &ListLearningsArgs,
    category_filter: Option<LearningCategory>,
) -> anyhow::Result<Value> {
    let active = load_active_memory(path);
    let reinforcements = Ledger::open(path)?.read_reinforcements()?;
    let mut entries: Vec<(String, Reinforcement, f64)> = reinforcements
        .learnings
        .into_iter()
        .filter_map(|(id, r)| {
            if let Some(filter) = category_filter {
                if r.category != filter {
                    return None;
                }
            }
            if cortex_core::confidence::is_contested(r.origin) {
                return None; // Contested facts are quarantined from retrieval.
            }
            let effective = confidence_with_spectral(&r, &id, active.as_ref());
            if effective < args.min_confidence {
                return None;
            }
            Some((id, r, effective))
        })
        .collect();
    entries.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    entries.truncate(args.limit);
    let mode = if active.is_some() {
        "spectral"
    } else {
        "scalar"
    };
    let results: Vec<Value> = entries
        .into_iter()
        .map(|(id, r, effective)| {
            let snippet: String = r.content.chars().take(100).collect();
            let mut entry = json!({
                "id": shortid(&id),
                "full_id": id,
                "category": category_value(r.category),
                "snippet": snippet,
                "confidence": round2(effective),
            });
            if args.show_decay {
                entry["effective_confidence"] = json!(round2(effective));
            }
            entry
        })
        .collect();
    Ok(json!({
        "learnings": results,
        "total": results.len(),
        "mode": mode,
    }))
}

pub async fn ledger_stats(server: &CortexServer, args: LedgerStatsArgs) -> anyhow::Result<Value> {
    // Stats describe one ledger: the first that exists (project of cwd, else global).
    let candidates = read_candidates(server, args.project_dir.as_deref())?;
    across(&candidates, ledger_stats_one, |_| true)
}

fn ledger_stats_one(path: &std::path::Path) -> anyhow::Result<Value> {
    let ledger = Ledger::open(path)?;
    let active = load_active_memory(path);
    let reinforcements = ledger.read_reinforcements()?;
    let mut by_category: Map<String, Value> = Map::new();
    let mut high = 0u64;
    let mut medium = 0u64;
    let mut low = 0u64;
    let total = reinforcements.learnings.len();
    for (id, r) in &reinforcements.learnings {
        let cat_key = category_value(r.category).to_string();
        let entry = by_category.entry(cat_key).or_insert(Value::from(0u64));
        if let Some(n) = entry.as_u64() {
            *entry = Value::from(n + 1);
        }
        let conf = confidence_with_spectral(r, id, active.as_ref());
        if conf >= 0.7 {
            high += 1;
        } else if conf >= 0.4 {
            medium += 1;
        } else {
            low += 1;
        }
    }
    let mode = if active.is_some() {
        "spectral"
    } else {
        "scalar"
    };
    Ok(json!({
        "exists": true,
        "path": path,
        "total_learnings": total,
        "by_category": Value::Object(by_category),
        "by_confidence": { "high": high, "medium": medium, "low": low },
        "confidence_mode": mode,
    }))
}

pub async fn tag_learning(server: &CortexServer, args: TagLearningArgs) -> anyhow::Result<Value> {
    let category = parse_category(&args.category)?;
    let confidence = args.confidence.clamp(0.0, 1.0);
    let mut content = args.content;
    if content.chars().count() > 500 {
        content = content.chars().take(500).collect();
    }
    let Some(path) = resolve_ledger(server, args.project_dir.as_deref())? else {
        return Ok(json!({"error": "Ledger path could not be resolved"}));
    };
    let ledger = Ledger::open(&path)?;
    let source = match args.source_file {
        Some(s) => Some(format!("mcp_tag:{s}")),
        None => Some("mcp_tag".to_string()),
    };
    let learning = Learning::new(category, content, confidence, source);
    let learning_id = learning.id.clone();
    let block = ledger.append_block("mcp-session", vec![learning], true)?;
    Ok(json!({
        "status": "created",
        "learning_id": shortid(&learning_id),
        "full_id": learning_id,
        "category": category_value(category),
        "confidence": confidence,
        "block_id": shortid(&block.id),
    }))
}

pub async fn get_session_summary(
    server: &CortexServer,
    args: GetSessionSummaryArgs,
) -> anyhow::Result<Value> {
    let candidates = read_candidates(server, args.project_dir.as_deref())?;
    across(
        &candidates,
        |path| session_summary_one(path, &args),
        |r| r["total"].as_u64().unwrap_or(0) > 0,
    )
}

fn session_summary_one(
    path: &std::path::Path,
    args: &GetSessionSummaryArgs,
) -> anyhow::Result<Value> {
    let ledger = Ledger::open(path)?;
    let index = ledger.read_index()?;
    // Group blocks by session_id, derive a lightweight summary per session.
    let mut sessions: Vec<(String, Vec<Block>)> = Vec::new();
    for entry in &index.blocks {
        if let Some(block) = ledger.read_block(&entry.id)? {
            if let Some(filter) = &args.session_id {
                if block.session_id != *filter {
                    continue;
                }
            }
            if let Some((_, blocks)) = sessions.iter_mut().find(|(s, _)| s == &block.session_id) {
                blocks.push(block);
            } else {
                sessions.push((block.session_id.clone(), vec![block]));
            }
        }
    }
    sessions.sort_by(|a, b| {
        let last_a = a.1.last().map(|b| b.timestamp.into_inner());
        let last_b = b.1.last().map(|b| b.timestamp.into_inner());
        last_b.cmp(&last_a)
    });
    sessions.truncate(args.limit);
    let summaries: Vec<Value> = sessions
        .into_iter()
        .map(|(session_id, blocks)| {
            let last_ts = blocks
                .last()
                .map(|b| b.timestamp.as_str())
                .unwrap_or_default();
            let learning_ids: Vec<String> = blocks
                .iter()
                .flat_map(|b| b.learnings.iter().map(|l| l.id.clone()))
                .take(10)
                .collect();
            let key_decisions: Vec<String> = blocks
                .iter()
                .flat_map(|b| b.learnings.iter())
                .filter(|l| matches!(l.category, LearningCategory::Decision))
                .map(|l| l.content.chars().take(120).collect::<String>())
                .take(5)
                .collect();
            let summary_text = blocks
                .iter()
                .flat_map(|b| b.learnings.iter().map(|l| l.content.as_str()))
                .collect::<Vec<_>>()
                .join(" | ");
            let trimmed: String = summary_text.chars().take(300).collect();
            json!({
                "session_id": session_id,
                "timestamp": last_ts,
                "summary_text": trimmed,
                "key_decisions": key_decisions,
                "files_discussed": Value::Array(vec![]),
                "learning_ids": learning_ids,
            })
        })
        .collect();
    Ok(json!({"summaries": summaries, "total": summaries.len()}))
}

// ===== handoff tools (v0.4.0) =====

/// Resolve the cortex-state directory for handoffs. Mirrors
/// `load_active_memory`'s convention: `<ledger>/cortex-state/`.
fn handoff_state_root(server: &CortexServer, project_dir: Option<&str>) -> anyhow::Result<PathBuf> {
    let ledger = resolve_ledger(server, project_dir)?
        .ok_or_else(|| anyhow!("no project ledger found and no global ledger available"))?;
    Ok(ledger.join("cortex-state"))
}

fn handoff_to_json(h: &cortex_handoff::Handoff) -> Value {
    json!({
        "handoff_id": h.handoff_id,
        "session_id": h.session_id,
        "timestamp": h.timestamp.as_str(),
        "completed_tasks": h.completed_tasks,
        "pending_tasks": h.pending_tasks,
        "blockers": h.blockers,
        "modified_files": h.modified_files,
        "context_notes": h.context_notes,
    })
}

pub async fn get_handoff(server: &CortexServer, args: GetHandoffArgs) -> anyhow::Result<Value> {
    // Default is the cwd's project ledger, then global (B4); never global first.
    let candidates = read_candidates(server, args.project_dir.as_deref())?;
    for (label, path) in &candidates {
        let state_root = path.join("cortex-state");
        let found = match args.session_id.as_deref() {
            Some(sid) => cortex_handoff::latest_for_session(&state_root, sid)?,
            None => cortex_handoff::read_current(&state_root)?,
        };
        if let Some(h) = found {
            return Ok(json!({ "handoff": handoff_to_json(&h), "ledger": label }));
        }
    }
    Ok(json!({
        "handoff": null,
        "ledger": null,
        "ledgers_searched": labels(&candidates),
        "note": "No handoff found. Use tag_handoff to record one at a pause-point.",
    }))
}

pub async fn tag_handoff(server: &CortexServer, args: TagHandoffArgs) -> anyhow::Result<Value> {
    if args.session_id.trim().is_empty() {
        return Err(anyhow!("session_id is required and must not be empty"));
    }
    let state_root = handoff_state_root(server, args.project_dir.as_deref())?;
    let handoff = cortex_handoff::Handoff::new(args.session_id)
        .with_completed(args.completed_tasks)
        .with_pending(args.pending_tasks)
        .with_blockers(args.blockers)
        .with_modified_files(args.modified_files)
        .with_context(args.context_notes);
    let path = cortex_handoff::record_handoff(&state_root, &handoff)?;
    Ok(json!({
        "handoff": handoff_to_json(&handoff),
        "stored_at": path.display().to_string(),
    }))
}

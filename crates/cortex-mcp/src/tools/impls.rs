//! Tool implementations. Each tool returns a JSON string. v2 returned `dict`
//! from Python; the rmcp Rust handler returns a `String` so we serialize the
//! response value into JSON ourselves to keep the wire format identical.

use std::path::PathBuf;

use anyhow::anyhow;
use chrono::Utc;
use cortex_core::confidence::decay_confidence;
use cortex_core::models::{
    Block, Learning, LearningCategory, OutcomeResult, Reinforcement, Reinforcements,
};
use cortex_core::Ledger;
use rmcp::ErrorData;
use serde_json::{json, Map, Value};

use super::args::*;
use crate::paths::{global_ledger_path, project_ledger_path};
use crate::server::CortexServer;

/// Convert an `anyhow::Result<Value>` into the rmcp tool string return type.
/// On success, serializes the value to a single-line JSON string. Application
/// errors (missing learnings, bad inputs) are reported as JSON `{"error": ...}`
/// payloads so the agent receives the same shape as the v2 Python tools.
pub async fn run(
    future: impl std::future::Future<Output = anyhow::Result<Value>>,
) -> Result<String, ErrorData> {
    let value = match future.await {
        Ok(v) => v,
        Err(e) => json!({ "error": e.to_string() }),
    };
    serde_json::to_string(&value)
        .map_err(|e| ErrorData::internal_error(format!("serialize tool result: {e}"), None))
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
    Ok(global_ledger_path())
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
    decay_confidence(
        reinforcement.confidence,
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

pub async fn search_learnings(
    server: &CortexServer,
    args: SearchLearningsArgs,
) -> anyhow::Result<Value> {
    let category_filter = match args.category.as_deref() {
        Some(c) => Some(parse_category(c)?),
        None => None,
    };
    let Some(path) = resolve_ledger(server, args.project_dir.as_deref())? else {
        return Ok(json!({"results": [], "total": 0, "error": null}));
    };
    let Some(_) = open_ledger(&path)? else {
        return Ok(json!({"results": [], "total": 0, "error": null}));
    };
    let active = load_active_memory(&path);
    let reinforcements = {
        let ledger = Ledger::open(&path)?;
        ledger.read_reinforcements()?
    };

    // Phase 7: spectral retrieval if active memory exists. Otherwise the
    // v3 substring path.
    let mut scored: Vec<(String, Reinforcement, f64, f64, &'static str)> = Vec::new();
    if let Some(snapshot) = &active {
        // Build a BM25 index over the corpus and score the query against it.
        let mut bm25 = cortex_similarity::Bm25Index::new();
        for r in reinforcements.learnings.values() {
            bm25.add(r.content_hash.clone(), &r.content);
        }
        bm25.recompute_stats();
        let query_scores: std::collections::HashMap<String, f64> =
            bm25.score_query(&args.query).into_iter().collect();
        let ranked = cortex_active_memory::spectral_query(snapshot, |node_id| {
            query_scores.get(&node_id.0).copied().unwrap_or(0.0)
        });
        for (entry, resonance) in ranked {
            // Find this entry's reinforcement by learning_id.
            let Some((id, r)) = reinforcements
                .learnings
                .iter()
                .find(|(id, _)| id.as_str() == entry.learning_id)
            else {
                continue;
            };
            if let Some(filter) = category_filter {
                if r.category != filter {
                    continue;
                }
            }
            let conf = confidence_with_spectral(r, id, active.as_ref());
            if conf < args.min_confidence {
                continue;
            }
            scored.push((id.clone(), r.clone(), conf, resonance, "spectral"));
        }
    } else {
        let needle = args.query.to_lowercase();
        for (id, r) in &reinforcements.learnings {
            if let Some(filter) = category_filter {
                if r.category != filter {
                    continue;
                }
            }
            if !needle.is_empty() && !r.content.to_lowercase().contains(&needle) {
                continue;
            }
            let conf = effective_confidence(r);
            if conf < args.min_confidence {
                continue;
            }
            scored.push((id.clone(), r.clone(), conf, 0.0, "substring"));
        }
        scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    }
    scored.truncate(args.limit);
    let mode = scored
        .first()
        .map(|(_, _, _, _, m)| *m)
        .unwrap_or("substring");
    let results: Vec<Value> = scored
        .iter()
        .enumerate()
        .map(|(rank, (id, r, conf, resonance, _))| {
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
    Ok(json!({
        "query": args.query,
        "category": args.category,
        "mode": mode,
        "results": results,
        "total": results.len(),
    }))
}

pub async fn get_learning(server: &CortexServer, args: GetLearningArgs) -> anyhow::Result<Value> {
    let Some(path) = resolve_ledger(server, args.project_dir.as_deref())? else {
        return Ok(json!({"error": "Ledger not found"}));
    };
    let Some(ledger) = open_ledger(&path)? else {
        return Ok(json!({"error": "Ledger not found"}));
    };
    let active = load_active_memory(&path);
    let reinforcements = ledger.read_reinforcements()?;
    let Some((id, reinforcement)) = match_prefix(&reinforcements, &args.learning_id) else {
        return Ok(json!({
            "error": format!("Learning '{}' not found", args.learning_id)
        }));
    };
    let block = block_for_reinforcement(&ledger, reinforcement);
    let mut result = Map::new();
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

pub async fn list_learnings(
    server: &CortexServer,
    args: ListLearningsArgs,
) -> anyhow::Result<Value> {
    let category_filter = match args.category.as_deref() {
        Some(c) => Some(parse_category(c)?),
        None => None,
    };
    let Some(path) = resolve_ledger(server, args.project_dir.as_deref())? else {
        return Ok(json!({"learnings": [], "total": 0}));
    };
    let Some(_) = open_ledger(&path)? else {
        return Ok(json!({"learnings": [], "total": 0}));
    };
    let active = load_active_memory(&path);
    let reinforcements = Ledger::open(&path)?.read_reinforcements()?;
    let mut entries: Vec<(String, Reinforcement, f64)> = reinforcements
        .learnings
        .into_iter()
        .filter_map(|(id, r)| {
            if let Some(filter) = category_filter {
                if r.category != filter {
                    return None;
                }
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
    let Some(path) = resolve_ledger(server, args.project_dir.as_deref())? else {
        return Ok(json!({"error": "Ledger not found", "exists": false}));
    };
    let Some(ledger) = open_ledger(&path)? else {
        return Ok(json!({"error": "Ledger not found", "exists": false, "path": path}));
    };
    let active = load_active_memory(&path);
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
    let Some((ledger, _)) = ledger_with_reinforcements(server, args.project_dir.as_deref())? else {
        return Ok(json!({"summaries": [], "total": 0}));
    };
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
    let state_root = handoff_state_root(server, args.project_dir.as_deref())?;
    let found = match args.session_id.as_deref() {
        Some(sid) => cortex_handoff::latest_for_session(&state_root, sid)?,
        None => cortex_handoff::read_current(&state_root)?,
    };
    match found {
        Some(h) => Ok(json!({ "handoff": handoff_to_json(&h) })),
        None => Ok(json!({
            "handoff": null,
            "note": "No handoff found. Use tag_handoff to record one at a pause-point.",
        })),
    }
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

// ===== deferred tools (substrate not yet ported) =====

const DEFERRED_NOTE: &str = "Feature pending v3.x port. v3 ships with the ledger substrate; the \
     entity graph and cross-project recommender are scheduled for \
     follow-on releases. v4's spectral retrieval (cortex-spectral crate) \
     subsumes much of this surface area.";

pub async fn get_suggestions(
    _server: &CortexServer,
    _args: GetSuggestionsArgs,
) -> anyhow::Result<Value> {
    Ok(json!({
        "suggestions": [],
        "total": 0,
        "error": DEFERRED_NOTE,
    }))
}

pub async fn entity_search(
    _server: &CortexServer,
    _args: EntitySearchArgs,
) -> anyhow::Result<Value> {
    Ok(json!({
        "results": [],
        "total": 0,
        "error": DEFERRED_NOTE,
    }))
}

pub async fn entity_show(_server: &CortexServer, _args: EntityShowArgs) -> anyhow::Result<Value> {
    Ok(json!({
        "error": DEFERRED_NOTE,
    }))
}

pub async fn entity_stats(_server: &CortexServer, _args: EntityStatsArgs) -> anyhow::Result<Value> {
    Ok(json!({
        "indexed": false,
        "error": DEFERRED_NOTE,
    }))
}

//! `cortex-post-tool-use` — fired after each tool call.
//!
//! Per v3 spec, applies a discovery-tagging post-condition: if the tool
//! result reveals new information about the codebase / API / pattern that
//! wasn't already in the ledger, surface a brief directive to capture it
//! via `tag_learning`. The hook itself does no extraction; it just nudges.
//!
//! ## Token economics (v0.4.0)
//!
//! Earlier versions emitted ~280 tokens of prose per fire. With many
//! tool calls per session — and especially with parallel external-MCP
//! calls — that adds up. v0.4.0 lands three orthogonal optimizations:
//!
//! 1. **Compressed directive (~60 tokens vs ~280)**. The pattern-not-
//!    state and context-not-snippet filters are kept; the explanatory
//!    prose is removed.
//! 2. **Result-aware skip**. The `tool_response` is now parsed from the
//!    hook input. Empty arrays, error payloads, and zero-hit search
//!    results suppress the nudge entirely.
//! 3. **Cross-process dedup window** via a sidecar at
//!    `<cache_dir>/cortex/hook-recent.json`. Same-tool fires within
//!    `DEDUP_WINDOW_SECS` are suppressed; parallel bursts collapse to a
//!    single nudge. The file is updated atomically (temp + rename).
//!
//! ## Filter (allowlist, v0.3.4)
//!
//! Only `WebFetch`, `WebSearch`, and external (non-cortex) `mcp__*`
//! tools fire. Routine I/O (Read/Write/Edit/Bash/Glob/Grep) and the
//! cortex MCP tools themselves are silent — the latter would otherwise
//! recurse on tag_learning calls.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use cortex_hooks::{read_input, write_output, HookInput};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEDUP_WINDOW_SECS: u64 = 30;
const DEDUP_FILE: &str = "cortex/hook-recent.json";

#[derive(Debug, Deserialize, Default)]
struct ToolEvent {
    #[serde(default)]
    tool_name: Option<String>,
    /// PostToolUse carries the tool result here. Shape is tool-specific
    /// — kept as a generic Value and inspected with heuristics.
    #[serde(default)]
    tool_response: Option<Value>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum NudgeKind {
    Web,
    ExternalMcp,
}

fn main() {
    let input = read_input();
    let event: ToolEvent =
        serde_json::from_value(Value::Object(input.extra.clone())).unwrap_or_default();
    let dedup = dedup_path();
    let context = build_directive(&input, &event, dedup.as_deref());
    write_output("PostToolUse", context);
}

fn build_directive(
    input: &HookInput,
    event: &ToolEvent,
    dedup_file: Option<&std::path::Path>,
) -> String {
    let tool = event.tool_name.as_deref().unwrap_or("");
    if tool.is_empty() {
        return String::new();
    }
    let Some(kind) = classify(tool) else {
        return String::new();
    };
    if let Some(ref response) = event.tool_response {
        if response_is_uninformative(response) {
            return String::new();
        }
    }
    if let Some(path) = dedup_file {
        if recently_fired(path, tool, input.session_id.as_deref()) {
            return String::new();
        }
        record_fire(path, tool, input.session_id.as_deref());
    }
    compressed_directive(tool, kind)
}

/// ~60-token directive (vs ~280 in v0.3.7). The pattern/state filter
/// and the snippet/context filter are retained; explanatory prose is
/// dropped. The handoff reminder is dropped from the per-fire body and
/// moved to the orientation skill (loaded once per session) so it
/// doesn't pay rent on every PostToolUse.
fn compressed_directive(tool: &str, kind: NudgeKind) -> String {
    let domain = match kind {
        NudgeKind::Web => "external docs / API contracts",
        NudgeKind::ExternalMcp => "service quirks / API patterns",
    };
    format!(
        "# Discovery-Tagging Nudge\n\
         `{tool}` ran. If its result revealed a durable pattern \
         ({domain}) not already in the ledger, capture via \
         `tag_learning`. Filters: **pattern not state** (will it be true \
         in a year?); **context not snippet** (43% snippet-distillation \
         failure rate). Pause-points → `tag_handoff` instead."
    )
}

/// Heuristic: detects empty / errored / zero-hit responses across the
/// shapes the cortex hook has seen in practice. Conservative — a
/// genuine result with `{"error": null, "results": [...]}` is NOT
/// suppressed because the array is non-empty.
fn response_is_uninformative(response: &Value) -> bool {
    match response {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty() || looks_like_zero_hit(s),
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => {
            // Top-level error payload — typical MCP shape.
            if let Some(err) = map.get("error") {
                if !err.is_null()
                    && !err.as_str().unwrap_or("").trim().is_empty()
                {
                    return true;
                }
            }
            // Common "results" / "matches" / "items" arrays — empty means
            // no signal worth nudging on.
            for key in ["results", "matches", "items", "entries", "hits"] {
                if let Some(Value::Array(arr)) = map.get(key) {
                    if arr.is_empty() {
                        return true;
                    }
                }
            }
            // Bare {"total": 0} pattern.
            if let Some(Value::Number(n)) = map.get("total") {
                if n.as_u64() == Some(0) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn looks_like_zero_hit(s: &str) -> bool {
    let lc = s.to_ascii_lowercase();
    matches!(
        lc.as_str(),
        "no results" | "no matches" | "not found" | "0 results"
    ) || lc.contains("no results found")
        || lc.contains("no matches found")
}

/// High-signal classification (v0.3.4 allowlist).
fn classify(tool: &str) -> Option<NudgeKind> {
    if matches!(tool, "WebFetch" | "WebSearch") {
        return Some(NudgeKind::Web);
    }
    if tool.starts_with("mcp__") && !tool.contains("claude-cortex") && !tool.contains("cortex") {
        return Some(NudgeKind::ExternalMcp);
    }
    None
}

// ===== dedup sidecar =====

#[derive(Debug, Default, Serialize, Deserialize)]
struct DedupState {
    /// Map of `<session>::<tool>` -> last-fire epoch seconds. Per-session
    /// scoping lets two concurrent Claude Code sessions in different
    /// terminals not accidentally suppress each other.
    #[serde(default)]
    entries: std::collections::BTreeMap<String, u64>,
}

fn dedup_path() -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("CORTEX_HOOK_DEDUP_PATH") {
        return Some(PathBuf::from(custom));
    }
    let cache = dirs_cache_dir()?;
    Some(cache.join(DEDUP_FILE))
}

/// Best-effort cross-platform cache dir. We avoid a `dirs` dependency
/// to keep the hook binary tiny — cold-start budget is <100ms.
fn dirs_cache_dir() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".cache"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn dedup_key(tool: &str, session: Option<&str>) -> String {
    format!("{}::{}", session.unwrap_or("_"), tool)
}

fn read_state(path: &std::path::Path) -> DedupState {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_state(path: &std::path::Path, state: &DedupState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec(state)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn recently_fired(path: &std::path::Path, tool: &str, session: Option<&str>) -> bool {
    let state = read_state(path);
    let key = dedup_key(tool, session);
    let Some(&last) = state.entries.get(&key) else {
        return false;
    };
    now_secs().saturating_sub(last) < DEDUP_WINDOW_SECS
}

fn record_fire(path: &std::path::Path, tool: &str, session: Option<&str>) {
    let mut state = read_state(path);
    let now = now_secs();
    state.entries.insert(dedup_key(tool, session), now);
    // Prune stale entries (>1 hour) on each write so the file doesn't
    // grow unbounded across long-lived sessions.
    state
        .entries
        .retain(|_, ts| now.saturating_sub(*ts) < 3600);
    let _ = write_state(path, &state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn input_with(session: &str, extra: Value) -> (HookInput, ToolEvent) {
        let mut i = HookInput::default();
        i.session_id = Some(session.to_string());
        if let Value::Object(map) = extra.clone() {
            i.extra = map;
        }
        let event: ToolEvent = serde_json::from_value(extra).unwrap_or_default();
        (i, event)
    }

    fn isolate_dedup_path() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("recent.json");
        (dir, path)
    }

    #[test]
    fn routine_tools_are_silent() {
        for t in [
            "Read", "Write", "Edit", "MultiEdit", "Bash", "Glob", "Grep",
            "TodoWrite", "TaskCreate", "TaskUpdate", "ToolSearch",
            "NotebookEdit",
        ] {
            assert!(classify(t).is_none(), "expected silence on {t}");
        }
    }

    #[test]
    fn web_tools_classify_as_web() {
        assert_eq!(classify("WebFetch"), Some(NudgeKind::Web));
        assert_eq!(classify("WebSearch"), Some(NudgeKind::Web));
    }

    #[test]
    fn external_mcp_classifies_as_external_mcp() {
        assert_eq!(
            classify("mcp__some_external__do_thing"),
            Some(NudgeKind::ExternalMcp)
        );
    }

    #[test]
    fn cortex_mcp_does_not_fire() {
        assert!(classify("mcp__plugin_claude-cortex_cortex__tag_learning").is_none());
    }

    #[test]
    fn empty_array_response_suppresses() {
        assert!(response_is_uninformative(&json!([])));
    }

    #[test]
    fn empty_results_field_suppresses() {
        assert!(response_is_uninformative(&json!({"results": []})));
        assert!(response_is_uninformative(&json!({"matches": []})));
        assert!(response_is_uninformative(&json!({"total": 0})));
    }

    #[test]
    fn error_payload_suppresses() {
        assert!(response_is_uninformative(
            &json!({"error": "rate limited"})
        ));
    }

    #[test]
    fn null_response_suppresses() {
        assert!(response_is_uninformative(&Value::Null));
    }

    #[test]
    fn populated_response_passes_through() {
        assert!(!response_is_uninformative(&json!({
            "results": [{"id": 1}],
            "total": 1
        })));
    }

    #[test]
    fn zero_hit_string_suppresses() {
        assert!(response_is_uninformative(&Value::String(
            "No results found.".into()
        )));
    }

    #[test]
    fn compressed_directive_keeps_filters_and_handoff_pointer() {
        let out = compressed_directive("WebFetch", NudgeKind::Web);
        assert!(out.contains("pattern not state"));
        assert!(out.contains("context not snippet"));
        assert!(out.contains("tag_handoff"));
        // Far smaller than the v0.3.7 directive (which was 1500+ chars).
        assert!(out.len() < 600, "directive grew to {} chars", out.len());
    }

    #[test]
    fn dedup_suppresses_repeat_within_window() {
        let (_dir, path) = isolate_dedup_path();
        let (input, event) = input_with(
            "sess-A",
            json!({"tool_name": "WebFetch", "tool_response": {"results": [{"x": 1}]}}),
        );
        let first = build_directive(&input, &event, Some(&path));
        assert!(!first.is_empty(), "first call should fire");
        let second = build_directive(&input, &event, Some(&path));
        assert!(second.is_empty(), "second call within window should suppress");
    }

    #[test]
    fn dedup_does_not_cross_sessions() {
        let (_dir, path) = isolate_dedup_path();
        let (input_a, event_a) = input_with(
            "sess-A",
            json!({"tool_name": "WebFetch", "tool_response": {"results": [{"x": 1}]}}),
        );
        let (input_b, event_b) = input_with(
            "sess-B",
            json!({"tool_name": "WebFetch", "tool_response": {"results": [{"x": 1}]}}),
        );
        let a = build_directive(&input_a, &event_a, Some(&path));
        let b = build_directive(&input_b, &event_b, Some(&path));
        assert!(!a.is_empty());
        assert!(!b.is_empty(), "different session should still fire");
    }

    #[test]
    fn dedup_does_not_suppress_different_tools() {
        let (_dir, path) = isolate_dedup_path();
        let (input_w, event_w) = input_with(
            "sess-A",
            json!({"tool_name": "WebFetch", "tool_response": {"results": [{"x": 1}]}}),
        );
        let (input_m, event_m) = input_with(
            "sess-A",
            json!({"tool_name": "mcp__github__list_issues", "tool_response": {"results": [{"x": 1}]}}),
        );
        assert!(!build_directive(&input_w, &event_w, Some(&path)).is_empty());
        assert!(!build_directive(&input_m, &event_m, Some(&path)).is_empty());
    }

    #[test]
    fn empty_result_does_not_record_dedup_entry() {
        // A suppressed-by-result fire should not "use up" the dedup
        // window — otherwise a zero-hit search would silence the next
        // populated call.
        let (_dir, path) = isolate_dedup_path();
        let (input_empty, event_empty) = input_with(
            "sess-A",
            json!({"tool_name": "WebSearch", "tool_response": {"results": []}}),
        );
        assert!(build_directive(&input_empty, &event_empty, Some(&path)).is_empty());
        let (input_real, event_real) = input_with(
            "sess-A",
            json!({"tool_name": "WebSearch", "tool_response": {"results": [{"x": 1}]}}),
        );
        assert!(
            !build_directive(&input_real, &event_real, Some(&path)).is_empty(),
            "follow-up populated call should fire"
        );
    }
}

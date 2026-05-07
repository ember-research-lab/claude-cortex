//! `cortex-post-tool-use` — fired after each tool call.
//!
//! Per v3 spec, applies a discovery-tagging post-condition: if the tool
//! result reveals new information about the codebase / API / pattern that
//! wasn't already in the ledger, surface a brief directive to capture it
//! via `tag_learning`. The hook itself does no extraction; it just nudges.
//!
//! v3.0.1: inverted the filter to an *allowlist* of high-signal tools.
//! v0.3.0 used a denylist (Read/Glob/Grep/Task*) which over-fired on
//! routine Bash/Write/Edit and even on the cortex MCP tools themselves
//! (creating a tag_learning recursion). The allowlist defaults to silence
//! and only fires on tools that frequently surface novel information
//! (WebFetch, WebSearch, certain external MCP tools).

use cortex_hooks::{read_input, write_output};
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
struct ToolEvent {
    #[serde(default)]
    tool_name: Option<String>,
}

fn main() {
    let input = read_input();
    let event: ToolEvent =
        serde_json::from_value(serde_json::Value::Object(input.extra.clone())).unwrap_or_default();
    let context = build_directive(&event);
    write_output("PostToolUse", context);
}

fn build_directive(event: &ToolEvent) -> String {
    let tool = event.tool_name.as_deref().unwrap_or("");
    if tool.is_empty() || !worth_nudging(tool) {
        return String::new();
    }

    let lines: Vec<String> = vec![
        "# Discovery-Tagging Post-Condition".to_string(),
        String::new(),
        format!(
            "Tool just ran: `{tool}`. If the result revealed something \
             non-obvious about this codebase, an external API, or a reusable \
             pattern that isn't already in the cortex ledger, capture it via \
             `tag_learning` with the appropriate category."
        ),
        String::new(),
        "Heuristic: tag if a future Claude session would benefit from \
         knowing this without re-running the tool. Skip routine output \
         (file lists, simple reads, expected results)."
            .to_string(),
    ];
    lines.join("\n")
}

/// High-signal tools whose output is *often* worth a discovery-tag nudge.
/// Most other tools (Bash, Write, Edit, Read, ToolSearch, the cortex MCP
/// tools themselves, Task*) are deliberately excluded — under v0.3.0's
/// denylist approach the hook over-fired and recursed on its own output.
fn worth_nudging(tool: &str) -> bool {
    if matches!(tool, "WebFetch" | "WebSearch") {
        return true;
    }
    // External MCP tools (third-party services) often surface novel info.
    // Cortex's own MCP tools (mcp__plugin_claude-cortex_*) are excluded
    // explicitly to prevent the tag_learning -> hook -> tag_learning loop.
    if tool.starts_with("mcp__") && !tool.contains("claude-cortex") && !tool.contains("cortex") {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routine_tools_are_silent() {
        for t in [
            "Read", "Write", "Edit", "MultiEdit", "Bash", "Glob", "Grep",
            "TodoWrite", "TaskCreate", "TaskUpdate", "ToolSearch", "NotebookEdit",
        ] {
            assert!(!worth_nudging(t), "expected silence on {t}");
        }
    }

    #[test]
    fn web_tools_fire() {
        assert!(worth_nudging("WebFetch"));
        assert!(worth_nudging("WebSearch"));
    }

    #[test]
    fn external_mcp_fires_but_cortex_mcp_does_not() {
        assert!(worth_nudging("mcp__some_external__do_thing"));
        assert!(!worth_nudging("mcp__plugin_claude-cortex_cortex__tag_learning"));
        assert!(!worth_nudging("mcp__plugin_claude-cortex_cortex__search_learnings"));
    }
}

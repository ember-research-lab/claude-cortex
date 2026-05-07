//! `cortex-post-tool-use` — fired after each tool call.
//!
//! Per v3 spec, applies a discovery-tagging post-condition: if the tool
//! result reveals new information about the codebase / API / pattern that
//! wasn't already in the ledger, surface a brief directive to capture it
//! via `tag_learning`. The hook itself does no extraction; it just nudges.

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
    let tool = event.tool_name.as_deref().unwrap_or("<tool>");
    if tool.is_empty() || skipworthy(tool) {
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

/// Tool names whose output is rarely worth tagging (high noise-to-signal).
fn skipworthy(tool: &str) -> bool {
    matches!(
        tool,
        "Read"
            | "Glob"
            | "Grep"
            | "TodoWrite"
            | "TaskCreate"
            | "TaskUpdate"
            | "TaskList"
            | "TaskGet"
    )
}

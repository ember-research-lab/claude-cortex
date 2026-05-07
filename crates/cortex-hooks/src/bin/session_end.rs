//! `cortex-session-end` — fired when a Claude Code session ends.
//!
//! v3.0.4: changed output mechanism. The hook output schema only allows
//! `hookSpecificOutput.additionalContext` for UserPromptSubmit / PostToolUse /
//! PostToolBatch — using it for SessionEnd fails strict schema validation
//! (the v0.3.3 binary tripped exactly that on real session exits).
//!
//! Cortex's SessionEnd directive is meant for the *user* (a reminder to
//! extract pending learnings + record outcomes) — it has no meaningful
//! agent-side effect because the agent is going away. So we print to
//! stderr instead of stdout. Stderr is shown to the user as a session-end
//! notice and bypasses JSON validation entirely. Stdout is left empty.

use cortex_hooks::read_input;

fn main() {
    let input = read_input();
    let directive = build_directive(&input);
    if !directive.is_empty() {
        eprintln!("{directive}");
    }
}

fn build_directive(input: &cortex_hooks::HookInput) -> String {
    let session = input
        .session_id
        .clone()
        .unwrap_or_else(|| "unknown-session".to_string());
    let lines: Vec<String> = vec![
        String::new(),
        "─── cortex session-end ───".to_string(),
        format!(
            "Session: {} (transcript: {})",
            session,
            input.transcript_path.as_deref().unwrap_or("<none>")
        ),
        String::new(),
        "If this session produced learnings worth keeping (discovery / decision \
         / error / pattern), persist them via `tag_learning` before closing. \
         For learnings looked up + applied during the session, call \
         `record_outcome` (success / partial / failure) so confidence \
         converges to reality."
            .to_string(),
        String::new(),
        "Skip if the session was purely conversational with no actionable \
         artifacts or tool calls."
            .to_string(),
        "──────────────────────────".to_string(),
    ];
    lines.join("\n")
}

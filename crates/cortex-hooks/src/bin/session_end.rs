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

use cortex_hooks::{project_dir, project_ledger_path, read_input};

fn main() {
    let input = read_input();
    let directive = build_directive(&input);
    if !directive.is_empty() {
        eprintln!("{directive}");
    }
    // Best-effort episodic capture: snapshot the tail so nothing is lost.
    if let Err(e) = capture_episode(&input) {
        eprintln!("cortex-session-end: capture failed (non-fatal): {e}");
    }
}

fn capture_episode(input: &cortex_hooks::HookInput) -> anyhow::Result<()> {
    let session_id = input
        .session_id
        .as_deref()
        .unwrap_or("unknown-session")
        .to_string();
    let transcript_path = input.transcript_path.as_deref();
    let reason = input
        .extra
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let capture_source = format!("sessionend:{reason}");

    let pd = project_dir(input).ok_or_else(|| anyhow::anyhow!("no cwd available"))?;
    let ledger_path = project_ledger_path(&pd);
    let state_root = ledger_path.join("cortex-state");

    cortex_episodic::capture_tail(
        &state_root,
        &session_id,
        transcript_path,
        &capture_source,
        None,
    )?;

    Ok(())
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

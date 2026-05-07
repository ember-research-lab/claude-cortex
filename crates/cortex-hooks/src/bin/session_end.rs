//! `cortex-session-end` — fired when a Claude Code session ends.
//!
//! Per v3 spec the hook itself does not parse the transcript; instead it
//! emits a directive instructing the orchestrator to extract any learnings
//! that might be useful and to invoke the outcome-recorder agent on
//! learnings referenced during the session. The directive is liberal on
//! purpose — under-extraction has proven to be the bigger failure mode.

use cortex_hooks::{read_input, write_output};

fn main() {
    let input = read_input();
    let context = build_directive(&input);
    write_output("SessionEnd", context);
}

fn build_directive(input: &cortex_hooks::HookInput) -> String {
    let session = input
        .session_id
        .clone()
        .unwrap_or_else(|| "unknown-session".to_string());
    let lines: Vec<String> = vec![
        "# Session-End Extraction Directive".to_string(),
        String::new(),
        format!(
            "Session: {} (transcript: {})",
            session,
            input.transcript_path.as_deref().unwrap_or("<none>")
        ),
        String::new(),
        "Liberal extraction: scan this conversation for any learning that \
         might be useful in a future session. Cast a wide net — \
         under-extraction has been the bigger failure mode. Tag each as \
         discovery / decision / error / pattern, set initial confidence in \
         [0.5, 0.8] depending on how directly it was demonstrated, and \
         persist via `tag_learning`."
            .to_string(),
        String::new(),
        "For learnings referenced during the session (i.e., looked up via \
         `search_learnings` / `get_learning` and applied), invoke the \
         outcome-recorder agent (or call `record_outcome` directly) so \
         confidence updates reflect what actually happened. Success / \
         partial / failure as appropriate; include a one-line context \
         describing how the learning was exercised."
            .to_string(),
        String::new(),
        "Skip this step only if the session was purely conversational with \
         no actionable artifacts or tool calls."
            .to_string(),
    ];
    lines.join("\n")
}

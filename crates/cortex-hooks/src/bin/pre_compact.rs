//! `cortex-pre-compact` — fired before Claude Code compacts a session transcript.
//!
//! Captures the current transcript tail into the episodic store so nothing
//! is lost when the transcript is truncated during compaction.
//!
//! This hook runs with `"async": true` so it is non-blocking and never delays
//! compaction. On any error we log to stderr and exit 0 — we must never
//! block compaction.

use cortex_hooks::{project_dir, project_ledger_path, read_input};

fn main() {
    let input = read_input();
    if let Err(e) = run(&input) {
        eprintln!("cortex-pre-compact: capture failed (non-fatal): {e}");
    }
}

fn run(input: &cortex_hooks::HookInput) -> anyhow::Result<()> {
    let session_id = input
        .session_id
        .as_deref()
        .unwrap_or("unknown-session")
        .to_string();
    let transcript_path = input.transcript_path.as_deref();

    // Derive state_root the same way cortex-dream does: ledger_path + "cortex-state".
    // For hooks the project ledger lives at <cwd>/.claude/cortex/ledger.
    let pd = project_dir(input).ok_or_else(|| anyhow::anyhow!("no cwd available"))?;
    let ledger_path = project_ledger_path(&pd);
    let state_root = ledger_path.join("cortex-state");

    // Read trigger field; absent / unknown defaults to "auto".
    let trigger = input
        .extra
        .get("trigger")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");
    let capture_source = match trigger {
        "manual" => "precompact:manual",
        _ => "precompact:auto",
    };

    let custom_instructions = input
        .extra
        .get("custom_instructions")
        .and_then(|v| v.as_str());

    cortex_episodic::capture_tail(
        &state_root,
        &session_id,
        transcript_path,
        capture_source,
        custom_instructions,
    )?;

    Ok(())
}

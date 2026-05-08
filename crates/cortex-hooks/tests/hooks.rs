//! End-to-end tests for the three hook binaries: pipe JSON in, parse JSON out.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use cortex_core::models::{Identity, LearningCategory};
use cortex_core::{Learning, Ledger};
use serde_json::Value;
use tempfile::TempDir;

fn binary(name: &str) -> std::path::PathBuf {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug")
        .join(name)
}

fn run_hook_raw(name: &str, env: &[(&str, &Path)], stdin: &str) -> (String, String) {
    let binary = binary(name);
    let mut cmd = Command::new(&binary);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", binary.display()));
    {
        let stdin_pipe = child.stdin.as_mut().unwrap();
        stdin_pipe.write_all(stdin.as_bytes()).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        output.status.success(),
        "{name} exited {:?}: {stderr}",
        output.status
    );
    (stdout, stderr)
}

fn run_hook(name: &str, env: &[(&str, &Path)], stdin: &str) -> Value {
    let (stdout, _) = run_hook_raw(name, env, stdin);
    if stdout.trim().is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("hook {name} bad json: {e}\n{stdout}"))
}

fn seed_project_ledger(dir: &Path) {
    let ledger_path = dir.join(".claude/cortex/ledger");
    std::fs::create_dir_all(&ledger_path).unwrap();
    let ledger = Ledger::open(&ledger_path).unwrap();
    ledger
        .key_manager()
        .generate_keypair(&Identity {
            name: "hook-test".into(),
            machine: "ci".into(),
            email: None,
        })
        .unwrap();
    let learnings = vec![
        Learning::new(
            LearningCategory::Pattern,
            "atomic writes use tempfile + rename inside a flock-held parent",
            0.85,
            None,
        ),
        Learning::new(
            LearningCategory::Discovery,
            "v3 substrate stores RFC3339 Z timestamps",
            0.75,
            None,
        ),
        Learning::new(
            LearningCategory::Decision,
            "match v2 sha256 hashing instead of switching to blake3",
            0.9,
            None,
        ),
    ];
    ledger.append_block("seed", learnings, true).unwrap();
}

#[test]
fn session_start_emits_orientation_and_learnings() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    seed_project_ledger(project.path());

    let stdin = serde_json::json!({
        "cwd": project.path().to_string_lossy(),
        "session_id": "test-session-1",
    })
    .to_string();
    let out = run_hook("cortex-session-start", &[("HOME", home.path())], &stdin);
    let context = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    // Orientation block is always present (auto-injected v0.4.0).
    assert!(context.contains("Cortex Orientation"));
    assert!(context.contains("orchestrator"));
    assert!(context.contains("substrate"));
    // Ledger block is present when there are learnings.
    assert!(context.contains("Prior Knowledge"));
    assert!(context.contains("Confidence interpretation"));
    assert!(context.contains("Top Learnings"));
    assert!(context.contains("atomic writes"));
}

#[test]
fn session_start_without_ledger_still_emits_orientation() {
    // v0.4.0: orientation is auto-injected regardless of ledger state.
    // Previously emitted nothing; the new contract is that orientation is
    // always present so the agent has the operating-mode directives even
    // on a brand-new project with no learnings yet.
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let stdin = serde_json::json!({
        "cwd": project.path().to_string_lossy(),
    })
    .to_string();
    let out = run_hook("cortex-session-start", &[("HOME", home.path())], &stdin);
    let context = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("orientation should always be emitted");
    assert!(context.contains("Cortex Orientation"));
    // No ledger learnings -> no Prior Knowledge block.
    assert!(!context.contains("Prior Knowledge"));
}

#[test]
fn session_end_emits_directive_to_stderr_with_empty_stdout() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let stdin = serde_json::json!({
        "cwd": project.path().to_string_lossy(),
        "session_id": "abc-123",
        "transcript_path": "/tmp/fake.jsonl",
    })
    .to_string();
    let (stdout, stderr) = run_hook_raw("cortex-session-end", &[("HOME", home.path())], &stdin);
    // SessionEnd output schema is strict (no hookSpecificOutput.additionalContext);
    // we print the directive to stderr and leave stdout empty.
    assert_eq!(stdout.trim(), "", "stdout should be empty for SessionEnd");
    assert!(stderr.contains("cortex session-end"));
    assert!(stderr.contains("abc-123"));
    assert!(stderr.contains("tag_learning"));
    assert!(stderr.contains("record_outcome"));
}

#[test]
fn post_tool_use_skips_routine_tools() {
    let home = TempDir::new().unwrap();
    let stdin = serde_json::json!({
        "tool_name": "Read",
    })
    .to_string();
    let out = run_hook("cortex-post-tool-use", &[("HOME", home.path())], &stdin);
    assert!(out.get("hookSpecificOutput").is_none());
}

#[test]
fn post_tool_use_emits_for_substantive_tools() {
    // Use a per-test dedup path so parallel test runs don't suppress
    // each other via the shared $HOME/.cache sidecar.
    let home = TempDir::new().unwrap();
    let dedup = home.path().join("dedup.json");
    let stdin = serde_json::json!({
        "tool_name": "WebFetch",
        "tool_response": {"results": [{"url": "https://example.com"}]},
    })
    .to_string();
    let out = run_hook(
        "cortex-post-tool-use",
        &[
            ("HOME", home.path()),
            ("CORTEX_HOOK_DEDUP_PATH", std::path::Path::new(&dedup)),
        ],
        &stdin,
    );
    let context = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("Discovery-Tagging Nudge"));
    assert!(context.contains("WebFetch"));
    assert!(context.contains("tag_handoff"));
    // Compressed (v0.4.0) — much shorter than the v0.3.7 directive.
    assert!(
        context.len() < 600,
        "directive grew to {} chars",
        context.len()
    );
}

#[test]
fn post_tool_use_suppresses_zero_hit_searches() {
    let home = TempDir::new().unwrap();
    let dedup = home.path().join("dedup-empty.json");
    let stdin = serde_json::json!({
        "tool_name": "WebSearch",
        "tool_response": {"results": []},
    })
    .to_string();
    let out = run_hook(
        "cortex-post-tool-use",
        &[
            ("HOME", home.path()),
            ("CORTEX_HOOK_DEDUP_PATH", std::path::Path::new(&dedup)),
        ],
        &stdin,
    );
    assert!(
        out.get("hookSpecificOutput").is_none(),
        "zero-hit search should not emit a nudge"
    );
}

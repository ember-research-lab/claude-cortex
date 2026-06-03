//! End-to-end tests for the four hook binaries: pipe JSON in, parse JSON out.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use cortex_core::models::{Identity, LearningCategory, OutcomeResult};
use cortex_core::{Learning, Ledger};
use cortex_episodic::episode::{EpisodeRecord, EpisodeStatus};
use cortex_episodic::manifest::manifest_path;
use cortex_episodic::manifest::save_manifest;
use cortex_episodic::EpisodeManifest;
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

/// Returns the state_root for a project dir, matching how the hook binaries
/// derive it: <project>/.claude/cortex/ledger/cortex-state.
fn state_root_for(project: &Path) -> std::path::PathBuf {
    project.join(".claude/cortex/ledger").join("cortex-state")
}

/// Write a minimal fake transcript file and return its path as a String.
fn write_fake_transcript(dir: &Path, content: &str) -> String {
    let path = dir.join("transcript.jsonl");
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

/// Load the EpisodeManifest from a project's state root.
fn load_manifest(project: &Path) -> EpisodeManifest {
    let sr = state_root_for(project);
    let path = manifest_path(&sr);
    if !path.is_file() {
        return EpisodeManifest::default();
    }
    let bytes = std::fs::read(&path).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn pre_compact_auto_creates_episode_with_correct_source() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let transcript = write_fake_transcript(
        project.path(),
        "fake transcript line 1\nfake transcript line 2\n",
    );

    let stdin = serde_json::json!({
        "cwd": project.path().to_string_lossy(),
        "session_id": "session-auto-1",
        "transcript_path": transcript,
        "trigger": "auto",
    })
    .to_string();

    let (stdout, _stderr) = run_hook_raw("cortex-pre-compact", &[("HOME", home.path())], &stdin);

    // async hook must emit NO stdout JSON
    assert_eq!(
        stdout.trim(),
        "",
        "pre_compact must emit no stdout (async hook)"
    );

    // Episode manifest must exist under the state root
    let manifest = load_manifest(project.path());
    assert_eq!(
        manifest.episodes.len(),
        1,
        "expected exactly 1 episode in manifest"
    );
    let episode = manifest.episodes.values().next().unwrap();
    assert_eq!(
        episode.capture_source, "precompact:auto",
        "capture_source must be precompact:auto"
    );
    assert_eq!(episode.session_id, "session-auto-1");
    assert!(episode.byte_range[1] > 0, "byte_range end should be > 0");
}

#[test]
fn pre_compact_manual_creates_episode_with_manual_source() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let transcript = write_fake_transcript(project.path(), "manual compaction transcript\n");

    let stdin = serde_json::json!({
        "cwd": project.path().to_string_lossy(),
        "session_id": "session-manual-1",
        "transcript_path": transcript,
        "trigger": "manual",
    })
    .to_string();

    let (stdout, _stderr) = run_hook_raw("cortex-pre-compact", &[("HOME", home.path())], &stdin);
    assert_eq!(stdout.trim(), "", "pre_compact must emit no stdout");

    let manifest = load_manifest(project.path());
    assert_eq!(manifest.episodes.len(), 1);
    let episode = manifest.episodes.values().next().unwrap();
    assert_eq!(
        episode.capture_source, "precompact:manual",
        "capture_source must be precompact:manual for trigger=manual"
    );
}

#[test]
fn pre_compact_replay_does_not_double_capture() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let transcript = write_fake_transcript(project.path(), "stable transcript content\n");

    let stdin = serde_json::json!({
        "cwd": project.path().to_string_lossy(),
        "session_id": "session-replay-1",
        "transcript_path": transcript,
        "trigger": "auto",
    })
    .to_string();

    // First run — should capture 1 episode.
    run_hook_raw("cortex-pre-compact", &[("HOME", home.path())], &stdin);

    // Second run with identical transcript — must be a no-op.
    run_hook_raw("cortex-pre-compact", &[("HOME", home.path())], &stdin);

    let manifest = load_manifest(project.path());
    assert_eq!(
        manifest.episodes.len(),
        1,
        "second run with same watermark must not create a duplicate episode"
    );
}

#[test]
fn session_end_capture_backstop() {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let transcript = write_fake_transcript(project.path(), "session end transcript backstop\n");

    let stdin = serde_json::json!({
        "cwd": project.path().to_string_lossy(),
        "session_id": "session-end-bs-1",
        "transcript_path": transcript,
        "reason": "normal",
    })
    .to_string();

    let (stdout, stderr) = run_hook_raw("cortex-session-end", &[("HOME", home.path())], &stdin);

    // stdout must remain empty (SessionEnd still uses stderr for directives)
    assert_eq!(stdout.trim(), "", "stdout must be empty for SessionEnd");
    // Existing directive must still appear
    assert!(
        stderr.contains("cortex session-end"),
        "directive must still appear in stderr"
    );

    // Episode must be captured
    let manifest = load_manifest(project.path());
    assert_eq!(
        manifest.episodes.len(),
        1,
        "session_end must write exactly 1 episode"
    );
    let episode = manifest.episodes.values().next().unwrap();
    assert!(
        episode.capture_source.starts_with("sessionend:"),
        "capture_source must start with 'sessionend:'; got: {}",
        episode.capture_source
    );
}

/// Seed a ledger with one learning, record a Success outcome on it, and
/// return `(learning_id, block_id)` so callers can wire up the episodic
/// manifest's `promoted_block_ids`.
fn seed_ledger_with_success_outcome(ledger_path: &std::path::Path) -> (String, String) {
    std::fs::create_dir_all(ledger_path).unwrap();
    let ledger = Ledger::open(ledger_path).unwrap();
    ledger
        .key_manager()
        .generate_keypair(&Identity {
            name: "eviction-test".into(),
            machine: "ci".into(),
            email: None,
        })
        .unwrap();
    let learning = Learning::new(
        LearningCategory::Pattern,
        "eviction integration test learning",
        0.85,
        None,
    );
    let learning_id = learning.id.clone();
    let block = ledger
        .append_block("eviction-session", vec![learning], true)
        .unwrap();
    let block_id = block.id.clone();
    ledger
        .record_outcome(&learning_id, OutcomeResult::Success, "integration test")
        .unwrap();
    (learning_id, block_id)
}

#[test]
fn session_start_run_eviction_prunes_confirmed_episode() {
    // Build: ledger with a Success outcome for block-X, episodic manifest with a
    // ConsolidatedPendingConfirmation episode whose promoted_block_ids = [block-X].
    // Run cortex-session-start; assert the episode file and manifest entry are gone.
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();

    let ledger_path = project.path().join(".claude/cortex/ledger");
    let (_learning_id, block_id) = seed_ledger_with_success_outcome(&ledger_path);

    // state_root follows hook convention: <ledger_path>/cortex-state
    let state_root = state_root_for(project.path());
    let episodic_dir = state_root.join("episodic");
    std::fs::create_dir_all(&episodic_dir).unwrap();

    // Build a ConsolidatedPendingConfirmation episode whose promoted block matches.
    let mut ep = EpisodeRecord::new("eviction-session", "precompact:auto", None, [0, 0]);
    ep.status = EpisodeStatus::ConsolidatedPendingConfirmation;
    ep.promoted_block_ids = vec![block_id];

    // Write the episode file to disk.
    let episode_file = episodic_dir.join(ep.filename());
    let ep_json = serde_json::to_vec(&ep).unwrap();
    std::fs::write(&episode_file, &ep_json).unwrap();

    // Write the manifest.
    let mut manifest = EpisodeManifest::default();
    manifest.episodes.insert(ep.episode_id.clone(), ep.clone());
    save_manifest(&state_root, &manifest).unwrap();

    // Verify the episode file exists before running the hook.
    assert!(
        episode_file.is_file(),
        "episode file should exist before running session_start"
    );

    // Run cortex-session-start with the project's cwd.
    let stdin = serde_json::json!({
        "cwd": project.path().to_string_lossy(),
        "session_id": "eviction-session",
    })
    .to_string();
    run_hook("cortex-session-start", &[("HOME", home.path())], &stdin);

    // Assert: the episode file should be gone (pruned by run_eviction).
    assert!(
        !episode_file.is_file(),
        "episode file should be deleted after session_start eviction"
    );

    // Assert: the manifest entry should be gone too.
    let manifest_after = load_manifest(project.path());
    assert!(
        !manifest_after.episodes.contains_key(&ep.episode_id),
        "manifest entry should be removed after session_start eviction"
    );
}

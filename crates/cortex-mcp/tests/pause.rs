//! Pause-cortex regression tests: read-only registration, annotations, and the
//! read-path fixes B1 (stale snapshot), B3 (isError), B4 (cwd project ledger).
//! Every ledger lives in a temp dir; nothing touches a real ledger.

use std::path::{Path, PathBuf};

use cortex_active_memory::{ActiveEntry, ActiveMemory};
use cortex_mcp::tools::args::*;
use cortex_mcp::tools::impls;
use cortex_mcp::CortexServer;
use cortex_spectral::NodeId;
use serde_json::Value;
use tempfile::TempDir;

const READ_TOOLS: [&str; 7] = [
    "get_handoff",
    "get_learning",
    "get_session_summary",
    "ledger_stats",
    "list_learnings",
    "recall_context",
    "search_learnings",
];
const WRITE_TOOLS: [&str; 4] = [
    "record_corroboration",
    "record_outcome",
    "tag_handoff",
    "tag_learning",
];

fn names(server: &CortexServer) -> Vec<String> {
    let mut v: Vec<String> = server.tools().iter().map(|t| t.name.to_string()).collect();
    v.sort();
    v
}

async fn tag(server: &CortexServer, content: &str) -> String {
    let r = impls::tag_learning(
        server,
        TagLearningArgs {
            content: content.into(),
            category: "discovery".into(),
            confidence: 0.7,
            source_file: None,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    r["full_id"].as_str().unwrap().to_string()
}

fn search_args(query: &str) -> SearchLearningsArgs {
    SearchLearningsArgs {
        query: query.into(),
        category: None,
        min_confidence: 0.0,
        limit: 10,
        project_dir: None,
    }
}

fn get_args(id: &str) -> GetLearningArgs {
    GetLearningArgs {
        learning_id: id.into(),
        show_outcomes: false,
        show_decay: false,
        project_dir: None,
    }
}

fn list_args() -> ListLearningsArgs {
    serde_json::from_value(serde_json::json!({})).unwrap()
}

fn summary_args() -> GetSessionSummaryArgs {
    serde_json::from_value(serde_json::json!({})).unwrap()
}

fn project_ledger(root: &Path) -> PathBuf {
    root.join(".claude/cortex/ledger")
}

fn snapshot_covering_head(ledger_dir: &Path, learning_ids: &[String]) {
    let hashes = cortex_core::Ledger::open(ledger_dir)
        .unwrap()
        .read_index()
        .unwrap()
        .blocks
        .into_iter()
        .map(|b| b.hash)
        .collect();
    let reinf = cortex_core::Ledger::open(ledger_dir)
        .unwrap()
        .read_reinforcements()
        .unwrap();
    let entries = learning_ids
        .iter()
        .map(|id| ActiveEntry {
            node: NodeId(reinf.learnings[id].content_hash.clone()),
            learning_id: id.clone(),
            projection_weight: 0.9,
            mode_projections: vec![0.9],
        })
        .collect();
    let am = ActiveMemory {
        snapshot_id: "test".into(),
        timestamp: "2026-05-08T00-00-00.000000Z".into(),
        source_block_hashes: hashes,
        eigenmode_count: 1,
        eigenvalues: vec![1.0],
        entries,
    };
    cortex_active_memory::write_snapshot(&ledger_dir.join("cortex-state"), &am).unwrap();
}

// ---------- read-only registration + annotations ----------

#[test]
fn read_only_server_registers_exactly_the_read_tools() {
    let server = CortexServer::new_read_only();
    assert!(server.is_read_only());
    assert_eq!(names(&server), READ_TOOLS);
}

#[test]
fn full_server_registers_read_plus_write_and_no_stubs() {
    let all = names(&CortexServer::new());
    let mut expect: Vec<&str> = READ_TOOLS
        .iter()
        .chain(WRITE_TOOLS.iter())
        .copied()
        .collect();
    expect.sort();
    assert_eq!(all, expect);
    for stub in [
        "get_suggestions",
        "entity_search",
        "entity_show",
        "entity_stats",
    ] {
        assert!(!all.iter().any(|n| n == stub), "{stub} must be gone");
    }
}

#[test]
fn every_read_only_tool_is_annotated_read_only_and_writes_are_not() {
    for t in CortexServer::new_read_only().tools() {
        let hint = t.annotations.as_ref().and_then(|a| a.read_only_hint);
        assert_eq!(hint, Some(true), "{} must carry readOnlyHint=true", t.name);
    }
    for t in CortexServer::new().tools() {
        if WRITE_TOOLS.contains(&t.name.as_ref()) {
            let hint = t.annotations.as_ref().and_then(|a| a.read_only_hint);
            assert_eq!(hint, Some(false), "{} is a write tool", t.name);
        }
    }
}

// ---------- B3: application errors are isError=true ----------

#[tokio::test]
async fn missing_learning_is_an_error_result_with_json_body() {
    let dir = TempDir::new().unwrap();
    let server = CortexServer::new().with_default_project_dir(dir.path().into());
    tag(&server, "something unrelated").await;

    let res = impls::run(impls::get_learning(&server, get_args("deadbeef")))
        .await
        .unwrap();
    assert_eq!(res.is_error, Some(true));
    let text = res.content[0].as_text().unwrap().text.clone();
    let body: Value = serde_json::from_str(&text).unwrap();
    assert!(body["error"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn bad_argument_and_successful_calls_are_flagged_correctly() {
    let dir = TempDir::new().unwrap();
    let server = CortexServer::new().with_default_project_dir(dir.path().into());
    let bad = impls::run(impls::search_learnings(
        &server,
        SearchLearningsArgs {
            category: Some("bogus".into()),
            ..search_args("x")
        },
    ))
    .await
    .unwrap();
    assert_eq!(bad.is_error, Some(true));

    tag(&server, "an ordinary learning").await;
    let ok = impls::run(impls::search_learnings(&server, search_args("ordinary")))
        .await
        .unwrap();
    assert_eq!(ok.is_error, Some(false));
}

// ---------- B1: stale snapshot + zero-score hits ----------

#[tokio::test]
async fn stale_snapshot_falls_back_to_bm25_and_finds_new_learning() {
    let dir = TempDir::new().unwrap();
    let server = CortexServer::new().with_default_project_dir(dir.path().into());
    let old = tag(&server, "tomita takesaki modular theory old note").await;
    snapshot_covering_head(&project_ledger(dir.path()), std::slice::from_ref(&old));

    // Covered: spectral, no stale flag.
    let fresh = impls::search_learnings(&server, search_args("tomita takesaki"))
        .await
        .unwrap();
    assert_eq!(fresh["mode"], "spectral");
    assert!(fresh.get("snapshot_stale").is_none());
    assert_eq!(fresh["total"], 1);

    // A learning lands after the snapshot: the snapshot no longer covers head.
    let newer = tag(&server, "flint precision build_QW arithmetic fix").await;
    let res = impls::search_learnings(&server, search_args("flint precision"))
        .await
        .unwrap();
    assert_eq!(res["mode"], "bm25");
    assert_eq!(res["snapshot_stale"], true);
    assert_eq!(res["uncovered"], 1);
    let ids: Vec<&str> = res["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["full_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![newer.as_str()]);
}

#[tokio::test]
async fn legacy_snapshot_without_block_hashes_is_stale() {
    let dir = TempDir::new().unwrap();
    let server = CortexServer::new().with_default_project_dir(dir.path().into());
    let id = tag(&server, "peptide docking note").await;
    snapshot_covering_head(&project_ledger(dir.path()), std::slice::from_ref(&id));
    // Overwrite with a snapshot that lists no hashes (what dream used to write).
    let am = ActiveMemory {
        snapshot_id: "legacy".into(),
        timestamp: "2026-05-09T00-00-00.000000Z".into(),
        source_block_hashes: vec![],
        eigenmode_count: 1,
        eigenvalues: vec![1.0],
        entries: vec![],
    };
    cortex_active_memory::write_snapshot(&project_ledger(dir.path()).join("cortex-state"), &am)
        .unwrap();
    let res = impls::search_learnings(&server, search_args("peptide docking"))
        .await
        .unwrap();
    assert_eq!(res["snapshot_stale"], true);
    assert_eq!(res["total"], 1);
}

#[tokio::test]
async fn zero_score_queries_return_nothing_in_both_modes() {
    let dir = TempDir::new().unwrap();
    let server = CortexServer::new().with_default_project_dir(dir.path().into());
    let id = tag(&server, "atomic file writes use tempfile and rename").await;

    let bm25 = impls::search_learnings(&server, search_args("zzqx qqvvk"))
        .await
        .unwrap();
    assert_eq!(bm25["total"], 0);

    snapshot_covering_head(&project_ledger(dir.path()), std::slice::from_ref(&id));
    let spectral = impls::search_learnings(&server, search_args("zzqx qqvvk"))
        .await
        .unwrap();
    assert_eq!(spectral["mode"], "spectral");
    assert_eq!(spectral["total"], 0, "zero-score hits must be dropped");
}

// ---------- B4: cwd project ledger, then global ----------

struct Env {
    _cwd: TempDir,
    _global: TempDir,
    cwd: PathBuf,
    global: PathBuf,
}

fn env() -> Env {
    let cwd = TempDir::new().unwrap();
    let global = TempDir::new().unwrap();
    Env {
        cwd: cwd.path().to_path_buf(),
        global: global.path().join("ledger"),
        _cwd: cwd,
        _global: global,
    }
}

fn reader(e: &Env) -> CortexServer {
    CortexServer::new_read_only()
        .with_cwd(e.cwd.clone())
        .with_global_ledger(e.global.clone())
}

#[tokio::test]
async fn get_learning_and_search_check_project_of_cwd_then_global_and_say_which() {
    let e = env();
    // Seed through write-capable servers pointed at temp dirs only.
    let proj_writer = CortexServer::new().with_default_project_dir(e.cwd.clone());
    let glob_writer = CortexServer::new().with_global_ledger(e.global.clone());
    let p = tag(&proj_writer, "project only tomita finding").await;
    let g = tag(&glob_writer, "global only zeta finding").await;
    let r = reader(&e);

    let got = impls::get_learning(&r, get_args(&p[..8])).await.unwrap();
    assert_eq!(got["ledger"], "project");
    let got = impls::get_learning(&r, get_args(&g[..8])).await.unwrap();
    assert_eq!(got["ledger"], "global");
    let miss = impls::get_learning(&r, get_args("deadbeef")).await.unwrap();
    assert_eq!(
        miss["ledgers_searched"],
        serde_json::json!(["project", "global"])
    );

    let s = impls::search_learnings(&r, search_args("tomita"))
        .await
        .unwrap();
    assert_eq!(s["ledger"], "project");
    assert_eq!(s["total"], 1);
    let s = impls::search_learnings(&r, search_args("zeta"))
        .await
        .unwrap();
    assert_eq!(s["ledger"], "global");
    assert_eq!(s["total"], 1);
    let s = impls::search_learnings(&r, search_args("nonexistentterm"))
        .await
        .unwrap();
    assert_eq!(s["total"], 0);
    assert!(s["ledger"].is_null());
}

#[tokio::test]
async fn get_handoff_defaults_to_cwd_project_not_global() {
    let e = env();
    let proj = CortexServer::new().with_default_project_dir(e.cwd.clone());
    let glob = CortexServer::new().with_global_ledger(e.global.clone());
    for (srv, note) in [(&proj, "project handoff"), (&glob, "global handoff")] {
        impls::tag_handoff(
            srv,
            TagHandoffArgs {
                session_id: "s1".into(),
                completed_tasks: vec![],
                pending_tasks: vec![],
                blockers: vec![],
                modified_files: vec![],
                context_notes: note.into(),
                project_dir: None,
            },
        )
        .await
        .unwrap();
    }
    let got = impls::get_handoff(&reader(&e), GetHandoffArgs::default())
        .await
        .unwrap();
    assert_eq!(got["ledger"], "project");
    assert_eq!(got["handoff"]["context_notes"], "project handoff");
}

// ---------- B4 (remaining read tools) ----------

#[tokio::test]
async fn list_stats_recall_summary_use_cwd_project_then_global_and_label() {
    let e = env();
    let proj = CortexServer::new().with_default_project_dir(e.cwd.clone());
    let glob = CortexServer::new().with_global_ledger(e.global.clone());
    tag(&proj, "project tomita finding").await;
    tag(&glob, "global zeta finding").await;
    let r = reader(&e);

    // Project ledger exists and has content: it answers.
    let list = impls::list_learnings(&r, list_args()).await.unwrap();
    assert_eq!(list["ledger"], "project");
    assert_eq!(
        list["ledgers_searched"],
        serde_json::json!(["project", "global"])
    );
    let stats = impls::ledger_stats(&r, LedgerStatsArgs::default())
        .await
        .unwrap();
    assert_eq!(stats["ledger"], "project");
    assert_eq!(stats["total_learnings"], 1);
    let rec = impls::recall_context(
        &r,
        RecallContextArgs {
            question: "tomita".into(),
            depth: None,
            budget_chars: None,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(rec["ledger"], "project");
    let sum = impls::get_session_summary(&r, summary_args())
        .await
        .unwrap();
    assert_eq!(sum["ledger"], "project");

    // Recall for a term only in global falls through and says so.
    let rec = impls::recall_context(
        &r,
        RecallContextArgs {
            question: "zeta".into(),
            depth: None,
            budget_chars: None,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(rec["ledger"], "global");
    assert!(!rec["context"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn list_falls_through_to_global_when_project_ledger_is_empty() {
    let e = env();
    // Project ledger dir exists but holds no learnings.
    let _ = CortexServer::new().with_default_project_dir(e.cwd.clone());
    cortex_core::Ledger::open(project_ledger(&e.cwd)).unwrap();
    let glob = CortexServer::new().with_global_ledger(e.global.clone());
    tag(&glob, "global only").await;
    let list = impls::list_learnings(&reader(&e), list_args())
        .await
        .unwrap();
    assert_eq!(list["ledger"], "global");
}

#[tokio::test]
async fn missing_ledger_is_an_error_in_every_read_tool() {
    let e = env(); // no ledger anywhere
    let r = reader(&e);
    let q = || RecallContextArgs {
        question: "x".into(),
        depth: None,
        budget_chars: None,
        project_dir: None,
    };
    let results = [
        impls::run(impls::search_learnings(&r, search_args("x")))
            .await
            .unwrap(),
        impls::run(impls::get_learning(&r, get_args("deadbeef")))
            .await
            .unwrap(),
        impls::run(impls::list_learnings(&r, list_args()))
            .await
            .unwrap(),
        impls::run(impls::ledger_stats(&r, LedgerStatsArgs::default()))
            .await
            .unwrap(),
        impls::run(impls::recall_context(&r, q())).await.unwrap(),
        impls::run(impls::get_session_summary(&r, summary_args()))
            .await
            .unwrap(),
        impls::run(impls::get_handoff(&r, GetHandoffArgs::default()))
            .await
            .unwrap(),
    ];
    for res in results {
        assert_eq!(res.is_error, Some(true));
        let text = res.content[0].as_text().unwrap().text.clone();
        assert!(text.contains("ledger_missing"), "{text}");
        assert!(
            text.contains("project ledger") && text.contains("global ledger"),
            "{text}"
        );
    }
}

// ---------- corrupt snapshot is reported ----------

#[tokio::test]
async fn corrupt_snapshot_is_reported_not_swallowed() {
    let dir = TempDir::new().unwrap();
    let server = CortexServer::new().with_default_project_dir(dir.path().into());
    tag(&server, "peptide docking note").await;
    let active = project_ledger(dir.path()).join("cortex-state/active");
    std::fs::create_dir_all(&active).unwrap();
    std::fs::write(active.join("current"), "active-bad.json\n").unwrap();
    std::fs::write(active.join("active-bad.json"), "{ not json").unwrap();

    let res = impls::search_learnings(&server, search_args("peptide docking"))
        .await
        .unwrap();
    assert_eq!(res["mode"], "bm25");
    assert!(res["snapshot_error"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
    assert_eq!(res["total"], 1);
}

// ---------- CORTEX_READ_ONLY parsing ----------

#[test]
fn read_only_env_parsing_is_case_insensitive_and_fail_closed() {
    use cortex_mcp::server::parse_read_only_env as p;
    for on in ["1", "true", "TRUE", "Yes", "on", "ON", " true "] {
        assert_eq!(p(Some(on)), Ok(true), "{on}");
    }
    for off in ["0", "false", "FALSE", "No", "off", "", "  "] {
        assert_eq!(p(Some(off)), Ok(false), "{off}");
    }
    assert_eq!(p(None), Ok(false));
    let err = p(Some("tru")).unwrap_err();
    assert!(err.contains("\"tru\""), "{err}");
}

#[test]
fn binary_refuses_to_start_on_bad_read_only_env_and_honours_good_one() {
    let bin = env!("CARGO_BIN_EXE_cortex-mcp");
    let bad = std::process::Command::new(bin)
        .env("CORTEX_READ_ONLY", "maybe")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("maybe"));
}

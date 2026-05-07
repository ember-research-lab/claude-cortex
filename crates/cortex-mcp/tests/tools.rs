//! Round-trip tests of the 12 MCP tools against a temporary project ledger.

use cortex_mcp::tools::args::*;
use cortex_mcp::tools::impls;
use cortex_mcp::CortexServer;
use serde_json::Value;
use tempfile::TempDir;

fn make_server(dir: &std::path::Path) -> CortexServer {
    CortexServer::new().with_default_project_dir(dir.to_path_buf())
}

#[tokio::test]
async fn tag_learning_then_search_then_get_then_record_outcome() {
    let dir = TempDir::new().unwrap();
    let server = make_server(dir.path());

    let tag = impls::tag_learning(
        &server,
        TagLearningArgs {
            content: "atomic writes use temp + rename inside a flock".into(),
            category: "pattern".into(),
            confidence: 0.7,
            source_file: Some("store.rs".into()),
            project_dir: None,
        },
    )
    .await
    .unwrap();
    let full_id = tag["full_id"].as_str().unwrap().to_string();
    assert_eq!(tag["status"], "created");
    assert_eq!(tag["category"], "pattern");

    let search = impls::search_learnings(
        &server,
        SearchLearningsArgs {
            query: "atomic writes".into(),
            category: None,
            min_confidence: 0.0,
            limit: 10,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    let results = search["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["full_id"], Value::String(full_id.clone()));

    let detail = impls::get_learning(
        &server,
        GetLearningArgs {
            learning_id: full_id[..8].to_string(),
            show_outcomes: true,
            show_decay: true,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(detail["id"], full_id);
    assert!(detail["outcomes"].as_array().unwrap().is_empty());
    assert!(detail.get("effective_confidence").is_some());

    let outcome = impls::record_outcome(
        &server,
        RecordOutcomeArgs {
            learning_id: full_id[..8].to_string(),
            result: "success".into(),
            comment: Some("validated atomic write path".into()),
            project_dir: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(outcome["status"], "recorded");
    let new_conf = outcome["new_confidence"].as_f64().unwrap();
    assert!(new_conf > 0.7, "expected confidence boost, got {new_conf}");
}

#[tokio::test]
async fn list_learnings_filters_by_category_and_confidence() {
    let dir = TempDir::new().unwrap();
    let server = make_server(dir.path());

    for (content, category, confidence) in [
        ("first discovery", "discovery", 0.8),
        ("second discovery", "discovery", 0.4),
        ("a decision", "decision", 0.9),
    ] {
        impls::tag_learning(
            &server,
            TagLearningArgs {
                content: content.into(),
                category: category.into(),
                confidence,
                source_file: None,
                project_dir: None,
            },
        )
        .await
        .unwrap();
    }

    let high_conf = impls::list_learnings(
        &server,
        ListLearningsArgs {
            min_confidence: 0.7,
            category: None,
            limit: 20,
            show_decay: false,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    let results = high_conf["learnings"].as_array().unwrap();
    assert_eq!(results.len(), 2, "expected high-confidence entries only");

    let only_decisions = impls::list_learnings(
        &server,
        ListLearningsArgs {
            min_confidence: 0.0,
            category: Some("decision".into()),
            limit: 20,
            show_decay: false,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    let results = only_decisions["learnings"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["category"], "decision");
}

#[tokio::test]
async fn ledger_stats_reports_counts_by_category_and_confidence() {
    let dir = TempDir::new().unwrap();
    let server = make_server(dir.path());

    for (content, cat, conf) in [
        ("a high pattern", "pattern", 0.85),
        ("a medium discovery", "discovery", 0.55),
        ("a low error", "error", 0.20),
    ] {
        impls::tag_learning(
            &server,
            TagLearningArgs {
                content: content.into(),
                category: cat.into(),
                confidence: conf,
                source_file: None,
                project_dir: None,
            },
        )
        .await
        .unwrap();
    }

    let stats = impls::ledger_stats(&server, LedgerStatsArgs { project_dir: None })
        .await
        .unwrap();
    assert_eq!(stats["exists"], true);
    assert_eq!(stats["total_learnings"], 3);
    let by_conf = &stats["by_confidence"];
    assert_eq!(by_conf["high"], 1);
    assert_eq!(by_conf["medium"], 1);
    assert_eq!(by_conf["low"], 1);
}

#[tokio::test]
async fn deferred_tools_return_pending_responses_with_correct_signatures() {
    let server = CortexServer::new();
    let cases = [
        impls::get_handoff(&server, GetHandoffArgs::default())
            .await
            .unwrap(),
        impls::get_suggestions(
            &server,
            GetSuggestionsArgs {
                limit: 5,
                min_confidence: 0.5,
                project_dir: None,
            },
        )
        .await
        .unwrap(),
        impls::entity_search(
            &server,
            EntitySearchArgs {
                query: "foo".into(),
                entity_type: None,
                limit: 20,
                project_dir: None,
            },
        )
        .await
        .unwrap(),
        impls::entity_show(
            &server,
            EntityShowArgs {
                qualified_name: "x".into(),
                show_dependencies: false,
                show_dependents: false,
                depth: 1,
                project_dir: None,
            },
        )
        .await
        .unwrap(),
        impls::entity_stats(&server, EntityStatsArgs::default())
            .await
            .unwrap(),
    ];
    for case in cases {
        assert!(
            case["error"].as_str().unwrap_or("").contains("pending"),
            "expected 'pending' note, got: {case}"
        );
    }
}

#[tokio::test]
async fn tag_learning_truncates_overlong_content() {
    let dir = TempDir::new().unwrap();
    let server = make_server(dir.path());

    let big = "a".repeat(800);
    let result = impls::tag_learning(
        &server,
        TagLearningArgs {
            content: big.clone(),
            category: "discovery".into(),
            confidence: 0.5,
            source_file: None,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    let full_id = result["full_id"].as_str().unwrap();
    let detail = impls::get_learning(
        &server,
        GetLearningArgs {
            learning_id: full_id.into(),
            show_outcomes: false,
            show_decay: false,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    let stored: &str = detail["content"].as_str().unwrap();
    assert_eq!(stored.chars().count(), 500);
}

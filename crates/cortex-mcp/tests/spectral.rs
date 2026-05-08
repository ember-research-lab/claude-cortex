//! Phase 6 + Phase 7 verification: spectral confidence and spectral retrieval
//! kick in when an active-memory snapshot exists; otherwise scalar fallback.

use cortex_active_memory::{ActiveEntry, ActiveMemory};
use cortex_mcp::tools::args::*;
use cortex_mcp::tools::impls;
use cortex_mcp::CortexServer;
use cortex_spectral::NodeId;
use std::path::Path;
use tempfile::TempDir;

fn make_server(dir: &Path) -> CortexServer {
    CortexServer::new().with_default_project_dir(dir.to_path_buf())
}

/// Seed the project ledger with five learnings: three about peptide
/// docking (a tight cluster), two about plugin schema (loosely related).
async fn seed(server: &CortexServer) -> Vec<String> {
    let ids = [
        ("pattern", "atomic file writes use tempfile and rename"),
        ("discovery", "PyRosetta ddG requires consistent repacking"),
        (
            "discovery",
            "PyRosetta peptide docking with large 100+ residues needs RDKit fragmentation",
        ),
        (
            "error",
            "marketplace.json plugin entries cannot have explicit version",
        ),
        (
            "pattern",
            "PyRosetta install via conda on WSL2 when pip wheel fails",
        ),
    ];
    let mut out = Vec::new();
    for (cat, content) in ids {
        let r = impls::tag_learning(
            server,
            TagLearningArgs {
                content: content.into(),
                category: cat.into(),
                confidence: 0.7,
                source_file: None,
                project_dir: None,
            },
        )
        .await
        .unwrap();
        out.push(r["full_id"].as_str().unwrap().to_string());
    }
    out
}

/// Write a minimal active-memory snapshot that boosts the PyRosetta
/// learnings (which are clustered) and ignores the plugin-schema entries.
fn seed_active_memory(state_dir: &Path, ids: &[String]) {
    let am = ActiveMemory {
        snapshot_id: "test".into(),
        timestamp: "2026-05-08T00-00-00.000000Z".into(),
        source_block_hashes: vec![],
        eigenmode_count: 1,
        eigenvalues: vec![1.0],
        // Three PyRosetta entries with high projection_weight, two
        // unrelated entries omitted from active memory.
        entries: vec![
            ActiveEntry {
                node: NodeId(format!("node-{}", &ids[1][..8])),
                learning_id: ids[1].clone(),
                projection_weight: 0.9,
                mode_projections: vec![0.9],
            },
            ActiveEntry {
                node: NodeId(format!("node-{}", &ids[2][..8])),
                learning_id: ids[2].clone(),
                projection_weight: 0.8,
                mode_projections: vec![0.8],
            },
            ActiveEntry {
                node: NodeId(format!("node-{}", &ids[4][..8])),
                learning_id: ids[4].clone(),
                projection_weight: 0.4,
                mode_projections: vec![0.4],
            },
        ],
    };
    let state_root = state_dir.join("cortex-state");
    cortex_active_memory::write_snapshot(&state_root, &am).unwrap();
}

#[tokio::test]
async fn spectral_confidence_overrides_scalar_when_snapshot_present() {
    let project = TempDir::new().unwrap();
    let server = make_server(project.path());
    let ids = seed(&server).await;

    // Without snapshot, all entries get scalar confidence (0.7 stored,
    // ~0.7 effective with no decay since they're brand new).
    let stats_pre = impls::ledger_stats(&server, LedgerStatsArgs::default())
        .await
        .unwrap();
    assert_eq!(stats_pre["confidence_mode"], "scalar");

    // Drop in an active-memory snapshot that boosts only some entries.
    let ledger_dir = project.path().join(".claude/cortex/ledger");
    seed_active_memory(&ledger_dir, &ids);

    let stats_post = impls::ledger_stats(&server, LedgerStatsArgs::default())
        .await
        .unwrap();
    assert_eq!(stats_post["confidence_mode"], "spectral");

    // get_learning with show_decay should report spectral mode for
    // entries in the snapshot, scalar for the others.
    let in_snapshot = impls::get_learning(
        &server,
        GetLearningArgs {
            learning_id: ids[1].clone(),
            show_outcomes: false,
            show_decay: true,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(in_snapshot["confidence_mode"], "spectral");
    // Top entry has projection_weight 0.9, max is 0.9 → spectral conf = 1.0
    assert!((in_snapshot["effective_confidence"].as_f64().unwrap() - 1.0).abs() < 1e-6);

    let out_of_snapshot = impls::get_learning(
        &server,
        GetLearningArgs {
            learning_id: ids[0].clone(), // "atomic writes" — not in snapshot
            show_outcomes: false,
            show_decay: true,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(out_of_snapshot["confidence_mode"], "scalar");
}

#[tokio::test]
async fn search_falls_back_to_substring_without_snapshot() {
    let project = TempDir::new().unwrap();
    let server = make_server(project.path());
    let _ids = seed(&server).await;

    let result = impls::search_learnings(
        &server,
        SearchLearningsArgs {
            query: "PyRosetta".into(),
            category: None,
            min_confidence: 0.0,
            limit: 10,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(result["mode"], "substring");
    let results = result["results"].as_array().unwrap();
    // 3 of the 5 entries mention PyRosetta.
    assert_eq!(results.len(), 3);
}

#[tokio::test]
async fn search_uses_spectral_mode_when_snapshot_present() {
    let project = TempDir::new().unwrap();
    let server = make_server(project.path());
    let ids = seed(&server).await;

    let ledger_dir = project.path().join(".claude/cortex/ledger");
    seed_active_memory(&ledger_dir, &ids);

    let result = impls::search_learnings(
        &server,
        SearchLearningsArgs {
            query: "PyRosetta peptide docking".into(),
            category: None,
            min_confidence: 0.0,
            limit: 10,
            project_dir: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(result["mode"], "spectral");
    let results = result["results"].as_array().unwrap();
    assert!(!results.is_empty());
    // Each result carries a resonance field (Phase 7 surface change).
    for r in results {
        assert!(r["resonance"].is_number());
    }
}

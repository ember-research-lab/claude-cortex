//! `cortex-session-start` — fired at the start of every Claude Code session.
//!
//! v0.4.0: orientation skill content is INJECTED directly via the
//! SessionStart hook. This decouples orientation availability from the
//! Skill-tool surfacing mechanism (which depends on plugin-loader
//! discovery quirks and trigger-phrase matching). The cortex-orientation
//! SKILL.md remains the single source of truth — it's embedded via
//! `include_str!` at compile time, so the hook output and the skill
//! body can never drift.
//!
//! v0.5.0 (Phase 3): if pending (unconsolidated) episodes exist in the
//! episodic store, appends a consolidation directive instructing the agent
//! to dispatch the `consolidator` agent before other work.
//!
//! Output structure (in order):
//!   1. Cortex Orientation directives (full skill body)
//!   2. Prior Knowledge from Cortex Ledger (top learnings + confidence
//!      interpretation)
//!   3. Consolidation Directive (only when pending episodes exist)

use std::path::Path;

use cortex_hooks::{
    collect_top_learnings, has_pending_episodes, pending_episode_ids, project_dir,
    project_ledger_path, read_input,
};

const PROJECT_MIN_CONF: f64 = 0.7;
const GLOBAL_MIN_CONF: f64 = 0.8;
const TOP_K: usize = 8;
/// Episodes older than this many days are evicted regardless of outcome
/// confirmation (TTL backstop).
const TTL_DAYS: u32 = 30;

/// Full cortex-orientation skill body, embedded at compile time. Single
/// source of truth: edit `skills/cortex-orientation/SKILL.md` and the
/// hook picks up the change on next rebuild.
const ORIENTATION_SKILL: &str = include_str!("../../../../skills/cortex-orientation/SKILL.md");

fn main() {
    let input = read_input();
    let project = project_dir(&input);
    let learnings = collect_top_learnings(project.as_deref(), PROJECT_MIN_CONF, GLOBAL_MIN_CONF);

    // Derive state_root: same convention as cortex-dream and pre_compact.
    let state_root = project
        .as_deref()
        .map(|pd| project_ledger_path(pd).join("cortex-state"));

    // Read source field from the extra map (may be absent on normal startup).
    let source = input
        .extra
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let context = build_context(&learnings, state_root.as_deref(), source);
    cortex_hooks::write_output("SessionStart", context);

    // Phase 5: lazy outcome-gated eviction.
    //
    // After the JSON output is flushed, run a best-effort reconcile-and-prune
    // step. Any I/O failure is silently ignored so SessionStart never crashes.
    // This step is idempotent: already-pruned episodes are a no-op.
    if let Some(ref sr) = state_root {
        run_eviction(sr, project.as_deref());
    }
}

/// Best-effort reconcile-and-prune. Silently returns on any error.
fn run_eviction(state_root: &Path, project_dir: Option<&Path>) {
    // Load the episodic manifest (missing manifest → nothing to evict).
    let mut manifest = match cortex_episodic::load_manifest(state_root) {
        Ok(m) => m,
        Err(_) => return,
    };

    // Load reinforcements from the project ledger.
    let reinforcements = project_dir
        .and_then(|pd| {
            let ledger_path = project_ledger_path(pd);
            cortex_core::Ledger::open(&ledger_path)
                .and_then(|l| l.read_reinforcements())
                .ok()
        })
        .unwrap_or_default();

    // Pure reconciliation: update episode statuses in-memory.
    manifest = cortex_episodic::reconcile_eviction(manifest, &reinforcements, TTL_DAYS);

    // Prune evictable episodes from disk and persist the updated manifest.
    let _ = cortex_episodic::prune_evictable(state_root, &mut manifest);
}

fn build_context(
    learnings: &[cortex_hooks::ScoredLearning],
    state_root: Option<&Path>,
    source: &str,
) -> String {
    let mut sections: Vec<String> = Vec::new();
    sections.push(orientation_block());
    if !learnings.is_empty() {
        sections.push(learnings_block(learnings));
    }
    if let Some(sr) = state_root {
        if let Some(directive) = consolidation_directive(sr, source) {
            sections.push(directive);
        }
    }
    sections.join("\n\n")
}

/// Returns a consolidation directive string if there are pending episodes,
/// or `None` when there are no pending episodes (zero-pending clean no-op).
fn consolidation_directive(state_root: &Path, source: &str) -> Option<String> {
    if !has_pending_episodes(state_root) {
        return None;
    }
    let ids = pending_episode_ids(state_root);
    let ids_list = if ids.is_empty() {
        String::from("(no episode IDs available)")
    } else {
        ids.iter()
            .map(|id| format!("  - {id}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let header = match source {
        "compact" | "clear" => {
            "# Consolidation Required (context lost)\n\n\
             Context was lost (compaction/clear). Pending episodes were captured — \
             consolidate them before other work:"
        }
        _ => {
            "# Consolidation Bench\n\n\
             Consolidation bench: tidy any pending episodes below:"
        }
    };

    Some(format!(
        "{header}\n\n\
         {ids_list}\n\n\
         Dispatch the `consolidator` agent over these episode IDs to promote \
         surviving learnings into the long-term ledger.\n\n\
         After the consolidator has finished promoting learnings via tag_learning, \
         run `/cortex-dream` to regenerate active memory so the new learnings are \
         reflected immediately."
    ))
}

fn orientation_block() -> String {
    // Strip the SKILL.md YAML frontmatter — keep only the body, since the
    // YAML keys (name/description/version) are metadata, not directives.
    let body = strip_frontmatter(ORIENTATION_SKILL);
    let mut out = String::new();
    out.push_str("# Cortex Orientation (auto-loaded)\n\n");
    out.push_str(
        "These directives establish how cortex-equipped sessions operate. They \
         are loaded automatically at session start; you do not need to invoke \
         them via the Skill tool.\n\n",
    );
    out.push_str(body.trim());
    out
}

fn strip_frontmatter(src: &str) -> &str {
    if let Some(rest) = src.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            return &rest[end + 5..];
        }
    }
    src
}

fn learnings_block(learnings: &[cortex_hooks::ScoredLearning]) -> String {
    let mut lines: Vec<String> = vec![
        "# Prior Knowledge from Cortex Ledger".to_string(),
        String::new(),
        "Before responding to any user request, scan the learnings below for \
         applicability to the current task. Apply directly when relevant, \
         and call `record_outcome` with success/partial/failure once a learning \
         is exercised so confidence converges to reality."
            .to_string(),
        String::new(),
        "Confidence interpretation: 0.85+ very high (apply by default unless \
         contradicted), 0.65-0.85 strong (apply with light verification), \
         0.50-0.65 hedged (use as a hint, verify before acting), <0.50 \
         (treat as unverified suggestion)."
            .to_string(),
        String::new(),
        "## Top Learnings".to_string(),
    ];
    for (i, l) in learnings.iter().take(TOP_K).enumerate() {
        let pct = (l.effective_confidence * 100.0).round() as u32;
        let id_short: String = l.id.chars().take(8).collect();
        lines.push(format!(
            "{}. [{} • {}% • {}] {}",
            i + 1,
            l.category,
            pct,
            id_short,
            l.content.trim()
        ));
    }
    lines.push(String::new());
    lines.push(
        "*Use `search_learnings`, `get_learning`, or `list_learnings` MCP tools \
         to explore the ledger further; record_outcome to update confidence.*"
            .to_string(),
    );
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_episodic::episode::{EpisodeRecord, EpisodeStatus};
    use cortex_episodic::manifest::{save_manifest, EpisodeManifest};
    use tempfile::TempDir;

    /// Seed a state_root with a manifest containing one episode of the given status.
    /// Returns (TempDir, episode_id).
    fn seed_manifest_with_episode(status: EpisodeStatus) -> (TempDir, String) {
        let tmp = TempDir::new().unwrap();
        let state_root = tmp.path();

        let mut episode = EpisodeRecord::new("test-session", "precompact:auto", None, [0, 0]);
        episode.status = status;
        let episode_id = episode.episode_id.clone();

        let mut manifest = EpisodeManifest::default();
        manifest.episodes.insert(episode_id.clone(), episode);

        std::fs::create_dir_all(state_root.join("episodic")).unwrap();
        save_manifest(state_root, &manifest).unwrap();

        (tmp, episode_id)
    }

    #[test]
    fn consolidation_directive_on_pending_episodes_compact_source() {
        let (tmp, episode_id) = seed_manifest_with_episode(EpisodeStatus::Unconsolidated);
        let context = build_context(&[], Some(tmp.path()), "compact");

        assert!(
            context.contains("context lost"),
            "should contain 'context lost' for source=compact; got:\n{context}"
        );
        assert!(
            context.contains("compaction/clear"),
            "should describe context-lost scenario"
        );
        assert!(
            context.contains(&episode_id),
            "should include the pending episode id; got:\n{context}"
        );
        assert!(
            context.contains("consolidator"),
            "should mention the consolidator agent"
        );
    }

    #[test]
    fn consolidation_directive_on_pending_episodes_startup_source() {
        let (tmp, episode_id) = seed_manifest_with_episode(EpisodeStatus::Unconsolidated);
        let context = build_context(&[], Some(tmp.path()), "startup");

        assert!(
            context.contains("Consolidation Bench") || context.contains("tidy any pending"),
            "should contain bench/tidy wording for source=startup; got:\n{context}"
        );
        assert!(
            context.contains(&episode_id),
            "should include the pending episode id; got:\n{context}"
        );
        assert!(
            context.contains("consolidator"),
            "should mention the consolidator agent"
        );
    }

    #[test]
    fn no_directive_on_zero_pending_episodes() {
        let (tmp, _episode_id) = seed_manifest_with_episode(EpisodeStatus::Evictable);
        let context = build_context(&[], Some(tmp.path()), "compact");

        assert!(
            !context.contains("consolidator"),
            "no consolidation directive when all episodes are Evictable; got:\n{context}"
        );
        assert!(
            !context.contains("Consolidation"),
            "no consolidation section when zero pending; got:\n{context}"
        );
    }

    #[test]
    fn no_directive_when_no_manifest() {
        let tmp = TempDir::new().unwrap();
        // No episodic dir or manifest at all.
        let context = build_context(&[], Some(tmp.path()), "compact");

        assert!(
            !context.contains("consolidator"),
            "no consolidation directive when no manifest exists; got:\n{context}"
        );
        assert!(
            !context.contains("Consolidation"),
            "no consolidation section when no manifest; got:\n{context}"
        );
    }

    #[test]
    fn session_start_includes_dream_directive_after_consolidation() {
        let (tmp, episode_id) = seed_manifest_with_episode(EpisodeStatus::Unconsolidated);
        let context = build_context(&[], Some(tmp.path()), "compact");

        // Must contain the consolidation directive.
        assert!(
            context.contains("consolidator"),
            "should mention the consolidator agent; got:\n{context}"
        );
        assert!(
            context.contains(&episode_id),
            "should include the pending episode id; got:\n{context}"
        );

        // Must also contain the dream re-index directive.
        assert!(
            context.contains("cortex-dream"),
            "should contain cortex-dream re-index directive; got:\n{context}"
        );
        assert!(
            context.contains("regenerate active memory"),
            "should explain why cortex-dream is invoked; got:\n{context}"
        );
    }

    #[test]
    fn no_dream_directive_on_zero_pending_episodes() {
        let (tmp, _episode_id) = seed_manifest_with_episode(EpisodeStatus::Evictable);
        let context = build_context(&[], Some(tmp.path()), "compact");

        assert!(
            !context.contains("cortex-dream"),
            "no cortex-dream directive when all episodes are non-pending; got:\n{context}"
        );
        assert!(
            !context.contains("consolidator"),
            "no consolidation directive when zero pending; got:\n{context}"
        );
    }
}

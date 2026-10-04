//! `cortex-session-start` — fired at the start of every Claude Code session.
//!
//! v0.4.0: orientation skill content is INJECTED directly via the
//! SessionStart hook. Since v0.6.0 the plugin no longer wires this hook
//! (cortex is paused; see CHANGELOG), and the orientation skill was removed;
//! the text lives on as `assets/orientation.md`, embedded via `include_str!`
//! so the binary keeps working if the hook is re-enabled.
//!
//! v0.5.0 (Phase 3): if pending (unconsolidated) episodes exist in the
//! episodic store, appends a consolidation directive instructing the agent
//! to consolidate them (since 0.6.0 the directive only reports that cortex is paused).
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

/// Orientation text, embedded at compile time from `assets/orientation.md`
/// (formerly the cortex-orientation skill).
const ORIENTATION_SKILL: &str = include_str!("../../assets/orientation.md");

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
    // step. Any I/O failure is logged to stderr (non-fatal) so SessionStart never crashes.
    // This step is idempotent: already-pruned episodes are a no-op.
    if let Some(ref sr) = state_root {
        run_eviction(sr, project.as_deref());
    }
}

/// Best-effort reconcile-and-prune. Logs non-fatal errors; never panics.
fn run_eviction(state_root: &Path, project_dir: Option<&Path>) {
    // Load the episodic manifest (missing manifest → nothing to evict).
    let mut manifest = match cortex_episodic::load_manifest(state_root) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "cortex-session-start: failed to load episodic manifest for eviction \
                 (non-fatal): {e}"
            );
            return;
        }
    };

    // Load reinforcements from the project ledger.
    let reinforcements = project_dir
        .and_then(|pd| {
            let ledger_path = project_ledger_path(pd);
            cortex_core::Ledger::open(&ledger_path)
                .and_then(|l| l.read_reinforcements())
                .inspect_err(|e| {
                    eprintln!(
                        "cortex-session-start: failed to read reinforcements for eviction \
                         (non-fatal, using empty): {e}"
                    );
                })
                .ok()
        })
        .unwrap_or_default();

    // Pure reconciliation: update episode statuses in-memory.
    manifest = cortex_episodic::reconcile_eviction(manifest, &reinforcements, TTL_DAYS);

    // Prune evictable episodes from disk and persist the updated manifest.
    if let Err(e) = cortex_episodic::prune_evictable(state_root, &mut manifest) {
        eprintln!("cortex-session-start: episode eviction failed (non-fatal): {e}");
    }
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
    // Cap the listing: 115 pending IDs were being injected into every session
    // start (~5 KB of UUIDs the model cannot act on). The session only
    // needs to know the set is non-empty.
    const MAX_LISTED: usize = 8;
    let ids_list = if ids.is_empty() {
        String::from("(no episode IDs available)")
    } else {
        let mut list = ids
            .iter()
            .take(MAX_LISTED)
            .map(|id| format!("  - {id}"))
            .collect::<Vec<_>>()
            .join("\n");
        if ids.len() > MAX_LISTED {
            list.push_str(&format!(
                "\n  - … and {} more ({} pending in total; see the episodic store)",
                ids.len() - MAX_LISTED,
                ids.len()
            ));
        }
        list
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
         cortex is paused (0.6.0): the consolidation agent and the dream command \
         were removed from the plugin, so nothing consolidates these episodes \
         automatically. Re-enable per the README before relying on them."
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
            context.contains("cortex is paused"),
            "should say cortex is paused"
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
            context.contains("cortex is paused"),
            "should say cortex is paused"
        );
    }

    #[test]
    fn no_directive_on_zero_pending_episodes() {
        let (tmp, _episode_id) = seed_manifest_with_episode(EpisodeStatus::Evictable);
        let context = build_context(&[], Some(tmp.path()), "compact");

        assert!(
            !context.contains("cortex is paused"),
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
            !context.contains("cortex is paused"),
            "no consolidation directive when no manifest exists; got:\n{context}"
        );
        assert!(
            !context.contains("Consolidation"),
            "no consolidation section when no manifest; got:\n{context}"
        );
    }

    #[test]
    fn session_start_names_no_removed_components() {
        let (tmp, episode_id) = seed_manifest_with_episode(EpisodeStatus::Unconsolidated);
        let context = build_context(&[], Some(tmp.path()), "compact");

        // Must contain the consolidation directive.
        assert!(
            context.contains("cortex is paused"),
            "should say cortex is paused; got:\n{context}"
        );
        assert!(
            context.contains(&episode_id),
            "should include the pending episode id; got:\n{context}"
        );

        // No stale reference to removed agents/commands.
        assert!(
            !context.contains("cortex-dream") && !context.contains("`consolidator`"),
            "must not name removed components; got:\n{context}"
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
            !context.contains("cortex is paused"),
            "no consolidation directive when zero pending; got:\n{context}"
        );
    }
}

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
//! Output structure (in order):
//!   1. Cortex Orientation directives (full skill body)
//!   2. Prior Knowledge from Cortex Ledger (top learnings + confidence
//!      interpretation)

use cortex_hooks::{collect_top_learnings, project_dir, read_input, write_output};

const PROJECT_MIN_CONF: f64 = 0.7;
const GLOBAL_MIN_CONF: f64 = 0.8;
const TOP_K: usize = 8;

/// Full cortex-orientation skill body, embedded at compile time. Single
/// source of truth: edit `skills/cortex-orientation/SKILL.md` and the
/// hook picks up the change on next rebuild.
const ORIENTATION_SKILL: &str = include_str!("../../../../skills/cortex-orientation/SKILL.md");

fn main() {
    let input = read_input();
    let project = project_dir(&input);
    let learnings = collect_top_learnings(project.as_deref(), PROJECT_MIN_CONF, GLOBAL_MIN_CONF);
    let context = build_context(&learnings);
    write_output("SessionStart", context);
}

fn build_context(learnings: &[cortex_hooks::ScoredLearning]) -> String {
    let mut sections: Vec<String> = Vec::new();
    sections.push(orientation_block());
    if !learnings.is_empty() {
        sections.push(learnings_block(learnings));
    }
    sections.join("\n\n")
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

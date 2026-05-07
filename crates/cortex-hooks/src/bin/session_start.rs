//! `cortex-session-start` — fired at the start of every Claude Code session.
//!
//! Surfaces top-confidence learnings from project + global ledgers (no
//! truncation, per the v3 spec) and prefixes a directive instructing the
//! agent to scan them for applicability before responding to the user.

use cortex_hooks::{collect_top_learnings, project_dir, read_input, write_output};

const PROJECT_MIN_CONF: f64 = 0.7;
const GLOBAL_MIN_CONF: f64 = 0.8;
const TOP_K: usize = 8;

fn main() {
    let input = read_input();
    let project = project_dir(&input);
    let learnings = collect_top_learnings(project.as_deref(), PROJECT_MIN_CONF, GLOBAL_MIN_CONF);
    let context = build_context(&learnings);
    write_output("SessionStart", context);
}

fn build_context(learnings: &[cortex_hooks::ScoredLearning]) -> String {
    if learnings.is_empty() {
        return String::new();
    }
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

//! Shared logic for cortex hook subprocess binaries.
//!
//! Hook protocol: read JSON from stdin, write JSON to stdout.
//! Cold-start budget: under 100ms.

use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::Utc;
use cortex_core::confidence::decay_confidence;
use cortex_core::models::Reinforcement;
use cortex_core::Ledger;
use serde::{Deserialize, Serialize};

const PROJECT_LEDGER_RELATIVE: &str = ".claude/cortex/ledger";
const GLOBAL_LEDGER_RELATIVE: &str = ".claude/ledger";

#[derive(Debug, Default, Deserialize)]
pub struct HookInput {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct HookOutput {
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: HookSpecificOutput,
}

#[derive(Debug, Serialize)]
pub struct HookSpecificOutput {
    #[serde(rename = "hookEventName")]
    pub hook_event_name: &'static str,
    #[serde(rename = "additionalContext")]
    pub additional_context: String,
}

pub fn read_input() -> HookInput {
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_err() {
        return HookInput::default();
    }
    serde_json::from_str(&buf).unwrap_or_default()
}

pub fn project_dir(input: &HookInput) -> Option<PathBuf> {
    input
        .cwd
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
}

pub fn project_ledger_path(project_dir: &Path) -> PathBuf {
    project_dir.join(PROJECT_LEDGER_RELATIVE)
}

pub fn global_ledger_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| Path::new(&h).join(GLOBAL_LEDGER_RELATIVE))
}

#[derive(Debug, Clone)]
pub struct ScoredLearning {
    pub id: String,
    pub category: String,
    pub content: String,
    pub stored_confidence: f64,
    pub effective_confidence: f64,
}

pub fn collect_top_learnings(
    project_dir: Option<&Path>,
    project_min: f64,
    global_min: f64,
) -> Vec<ScoredLearning> {
    let mut out = Vec::new();
    if let Some(pd) = project_dir {
        let path = project_ledger_path(pd);
        if path.is_dir() {
            extend_from_ledger(&path, project_min, &mut out);
        }
    }
    if let Some(global) = global_ledger_path() {
        if global.is_dir() {
            extend_from_ledger(&global, global_min, &mut out);
        }
    }
    out.sort_by(|a, b| {
        b.effective_confidence
            .partial_cmp(&a.effective_confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn extend_from_ledger(path: &Path, min_conf: f64, out: &mut Vec<ScoredLearning>) {
    let Ok(ledger) = Ledger::open(path) else {
        return;
    };
    let Ok(reinforcements) = ledger.read_reinforcements() else {
        return;
    };
    for (id, r) in reinforcements.learnings {
        let effective = effective_confidence(&r);
        if effective < min_conf {
            continue;
        }
        out.push(ScoredLearning {
            id,
            category: category_str(r.category).to_string(),
            content: r.content,
            stored_confidence: r.confidence,
            effective_confidence: effective,
        });
    }
}

fn effective_confidence(r: &Reinforcement) -> f64 {
    decay_confidence(r.confidence, r.last_applied.into_inner(), Utc::now())
}

fn category_str(c: cortex_core::models::LearningCategory) -> &'static str {
    use cortex_core::models::LearningCategory::*;
    match c {
        Discovery => "discovery",
        Decision => "decision",
        Error => "error",
        Pattern => "pattern",
    }
}

pub fn write_output(event: &'static str, additional_context: String) {
    if additional_context.is_empty() {
        return;
    }
    let output = HookOutput {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: event,
            additional_context,
        },
    };
    if let Ok(s) = serde_json::to_string(&output) {
        println!("{s}");
    }
}

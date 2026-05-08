//! Argument structs for the 12 MCP tools. Field names and shapes match v2's
//! Python signatures so existing usage patterns continue to work.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchLearningsArgs {
    /// Full-text search query.
    pub query: String,
    /// Filter by category: discovery, decision, error, pattern.
    #[serde(default)]
    pub category: Option<String>,
    /// Minimum confidence threshold (0.0 - 1.0).
    #[serde(default = "default_min_confidence_search")]
    pub min_confidence: f64,
    /// Maximum number of results.
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    /// Project directory for project-specific search, or null for global.
    #[serde(default)]
    pub project_dir: Option<String>,
}

fn default_min_confidence_search() -> f64 {
    0.5
}

fn default_search_limit() -> usize {
    10
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetLearningArgs {
    /// Full or partial learning ID (prefix match supported).
    pub learning_id: String,
    /// Include outcome history.
    #[serde(default)]
    pub show_outcomes: bool,
    /// Include effective confidence with decay calculation.
    #[serde(default)]
    pub show_decay: bool,
    /// Project directory, or null for global ledger.
    #[serde(default)]
    pub project_dir: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RecordOutcomeArgs {
    /// Learning ID to update.
    pub learning_id: String,
    /// Outcome result: "success", "partial", or "failure".
    pub result: String,
    /// Optional context about the outcome.
    #[serde(default)]
    pub comment: Option<String>,
    /// Project directory, or null for global ledger.
    #[serde(default)]
    pub project_dir: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListLearningsArgs {
    /// Minimum confidence threshold (0.0 - 1.0).
    #[serde(default = "default_min_confidence_list")]
    pub min_confidence: f64,
    /// Filter by category: discovery, decision, error, pattern.
    #[serde(default)]
    pub category: Option<String>,
    /// Maximum results.
    #[serde(default = "default_list_limit")]
    pub limit: usize,
    /// Include effective confidence with decay.
    #[serde(default)]
    pub show_decay: bool,
    /// Project directory, or null for global ledger.
    #[serde(default)]
    pub project_dir: Option<String>,
}

fn default_min_confidence_list() -> f64 {
    0.5
}

fn default_list_limit() -> usize {
    20
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct LedgerStatsArgs {
    /// Project directory, or null for global ledger.
    #[serde(default)]
    pub project_dir: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TagLearningArgs {
    /// The learning content (max 500 chars).
    pub content: String,
    /// Category: discovery, decision, error, pattern.
    pub category: String,
    /// Initial confidence (0.0 - 1.0).
    #[serde(default = "default_tag_confidence")]
    pub confidence: f64,
    /// Optional source file reference.
    #[serde(default)]
    pub source_file: Option<String>,
    /// Project directory, or null for global ledger.
    #[serde(default)]
    pub project_dir: Option<String>,
}

fn default_tag_confidence() -> f64 {
    0.7
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetSessionSummaryArgs {
    /// Specific session ID, or null for most recent.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Number of summaries to return.
    #[serde(default = "default_summary_limit")]
    pub limit: usize,
    /// Project directory.
    #[serde(default)]
    pub project_dir: Option<String>,
}

fn default_summary_limit() -> usize {
    3
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct GetHandoffArgs {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub project_dir: Option<String>,
}

/// Record a fresh handoff capturing pause-point state.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TagHandoffArgs {
    /// Session identity. Use the Claude Code session id if available; any
    /// stable string works for grouping multiple pauses from the same
    /// session.
    pub session_id: String,
    /// Tasks completed during this session (or since last handoff).
    #[serde(default)]
    pub completed_tasks: Vec<String>,
    /// Tasks still open at pause-point.
    #[serde(default)]
    pub pending_tasks: Vec<String>,
    /// Anything blocking forward progress (waiting on credentials, design
    /// decision, etc).
    #[serde(default)]
    pub blockers: Vec<String>,
    /// Files modified during this session (reproduces what `git status`
    /// would show; lets the next session orient quickly).
    #[serde(default)]
    pub modified_files: Vec<String>,
    /// Free-form context. Where we paused, why, and any decision that
    /// would otherwise be lost.
    #[serde(default)]
    pub context_notes: String,
    #[serde(default)]
    pub project_dir: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetSuggestionsArgs {
    #[serde(default = "default_suggestions_limit")]
    pub limit: usize,
    #[serde(default = "default_min_confidence_search")]
    pub min_confidence: f64,
    #[serde(default)]
    pub project_dir: Option<String>,
}

fn default_suggestions_limit() -> usize {
    5
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EntitySearchArgs {
    pub query: String,
    #[serde(default)]
    pub entity_type: Option<String>,
    #[serde(default = "default_entity_search_limit")]
    pub limit: usize,
    #[serde(default)]
    pub project_dir: Option<String>,
}

fn default_entity_search_limit() -> usize {
    20
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EntityShowArgs {
    pub qualified_name: String,
    #[serde(default)]
    pub show_dependencies: bool,
    #[serde(default)]
    pub show_dependents: bool,
    #[serde(default = "default_entity_depth")]
    pub depth: u32,
    #[serde(default)]
    pub project_dir: Option<String>,
}

fn default_entity_depth() -> u32 {
    1
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct EntityStatsArgs {
    #[serde(default)]
    pub project_dir: Option<String>,
}

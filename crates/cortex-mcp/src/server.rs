//! rmcp `ServerHandler` exposing the cortex tools over stdio.
//!
//! Read tools and write tools live in separate routers. A read-only server
//! (`--read-only` / `CORTEX_READ_ONLY=1`) registers only the read router, so a
//! write tool that is added later cannot leak into a read-only instance.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};

use crate::tools::{
    args::{
        GetHandoffArgs, GetLearningArgs, GetSessionSummaryArgs, LedgerStatsArgs, ListLearningsArgs,
        RecallContextArgs, RecordCorroborationArgs, RecordOutcomeArgs, SearchLearningsArgs,
        TagHandoffArgs, TagLearningArgs,
    },
    impls,
};

/// Parse a `CORTEX_READ_ONLY` value (fail-closed on anything unrecognised).
/// `None`/empty and 0/false/no/off mean writable; 1/true/yes/on (any case) mean
/// read-only; any other value is an error naming it, so a typo can never leave
/// a server silently writable.
pub fn parse_read_only_env(value: Option<&str>) -> Result<bool, String> {
    let Some(raw) = value else { return Ok(false) };
    let v = raw.trim().to_ascii_lowercase();
    match v.as_str() {
        "" | "0" | "false" | "no" | "off" => Ok(false),
        "1" | "true" | "yes" | "on" => Ok(true),
        _ => Err(format!(
            "invalid CORTEX_READ_ONLY value {raw:?}: use 1/true/yes/on or 0/false/no/off"
        )),
    }
}

/// Server state shared across tool handlers.
#[derive(Clone)]
pub struct CortexServer {
    /// Optional default project directory. When provided, all `project_dir`
    /// arguments default to this. Mostly useful for tests.
    pub default_project_dir: Option<PathBuf>,
    /// Test seam: stands in for the process cwd when resolving the project ledger.
    pub(crate) cwd_override: Option<PathBuf>,
    /// Test seam: stands in for `~/.claude/ledger`.
    pub(crate) global_ledger_override: Option<PathBuf>,
    read_only: bool,
    pub(crate) tool_router: ToolRouter<Self>,
}

impl CortexServer {
    /// Full server: read and write tools.
    pub fn new() -> Self {
        Self::build(false)
    }

    /// Read-only server: write tools are not registered at all.
    pub fn new_read_only() -> Self {
        Self::build(true)
    }

    fn build(read_only: bool) -> Self {
        let mut tool_router = Self::read_router();
        if !read_only {
            tool_router.merge(Self::write_router());
        }
        Self {
            default_project_dir: None,
            cwd_override: None,
            global_ledger_override: None,
            read_only,
            tool_router,
        }
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// The tools this server registers (what `tools/list` returns).
    pub fn tools(&self) -> Vec<Tool> {
        self.tool_router.list_all()
    }

    pub fn with_default_project_dir(mut self, dir: PathBuf) -> Self {
        self.default_project_dir = Some(dir);
        self
    }

    /// Test seam: treat `dir` as the process cwd.
    pub fn with_cwd(mut self, dir: PathBuf) -> Self {
        self.cwd_override = Some(dir);
        self
    }

    /// Test seam: use `ledger` as the global ledger instead of `~/.claude/ledger`.
    pub fn with_global_ledger(mut self, ledger: PathBuf) -> Self {
        self.global_ledger_override = Some(ledger);
        self
    }

    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }
}

impl Default for CortexServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router(router = read_router)]
impl CortexServer {
    /// Search the knowledge ledger using full-text (substring) match.
    #[tool(
        name = "search_learnings",
        annotations(read_only_hint = true),
        description = "Search the knowledge ledger using full-text search."
    )]
    pub async fn search_learnings(
        &self,
        Parameters(args): Parameters<SearchLearningsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        impls::run(impls::search_learnings(self, args)).await
    }

    /// Recall a budget-bounded, relationship-aware context subgraph of relevant learnings.
    #[tool(
        name = "recall_context",
        annotations(read_only_hint = true),
        description = "Recall a budget-bounded, relationship-aware context subgraph of relevant learnings for a question."
    )]
    pub async fn recall_context(
        &self,
        Parameters(args): Parameters<RecallContextArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        impls::run(impls::recall_context(self, args)).await
    }

    /// Get full details of a specific learning by ID.
    #[tool(
        name = "get_learning",
        annotations(read_only_hint = true),
        description = "Get full details of a specific learning by ID."
    )]
    pub async fn get_learning(
        &self,
        Parameters(args): Parameters<GetLearningArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        impls::run(impls::get_learning(self, args)).await
    }

    /// List learnings from the ledger sorted by confidence.
    #[tool(
        name = "list_learnings",
        annotations(read_only_hint = true),
        description = "List learnings from the ledger sorted by confidence."
    )]
    pub async fn list_learnings(
        &self,
        Parameters(args): Parameters<ListLearningsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        impls::run(impls::list_learnings(self, args)).await
    }

    /// Get statistics about the knowledge ledger.
    #[tool(
        name = "ledger_stats",
        annotations(read_only_hint = true),
        description = "Get statistics about the knowledge ledger."
    )]
    pub async fn ledger_stats(
        &self,
        Parameters(args): Parameters<LedgerStatsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        impls::run(impls::ledger_stats(self, args)).await
    }

    /// Get recent session summaries derived from ledger blocks.
    #[tool(
        name = "get_session_summary",
        annotations(read_only_hint = true),
        description = "Get recent session summaries for context."
    )]
    pub async fn get_session_summary(
        &self,
        Parameters(args): Parameters<GetSessionSummaryArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        impls::run(impls::get_session_summary(self, args)).await
    }

    /// Get the latest work-in-progress handoff for session continuity.
    #[tool(
        name = "get_handoff",
        annotations(read_only_hint = true),
        description = "Get the latest work-in-progress handoff for session continuity. \
                       With session_id, returns the latest handoff for that session; \
                       without, returns the most recent handoff across all sessions."
    )]
    pub async fn get_handoff(
        &self,
        Parameters(args): Parameters<GetHandoffArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        impls::run(impls::get_handoff(self, args)).await
    }
}

/// Mutating tools. Registered ONLY when the server is not read-only; they live
/// in their own router so a read-only server never even constructs them.
#[tool_router(router = write_router)]
impl CortexServer {
    /// Record outcome for a learning (updates confidence via reinforcement).
    #[tool(
        name = "record_outcome",
        annotations(read_only_hint = false),
        description = "Record outcome for a learning (updates confidence via reinforcement)."
    )]
    pub async fn record_outcome(
        &self,
        Parameters(args): Parameters<RecordOutcomeArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        impls::run(impls::record_outcome(self, args)).await
    }

    /// Record an independent re-observation of a learning (corroboration, not an outcome).
    #[tool(
        name = "record_corroboration",
        annotations(read_only_hint = false),
        description = "Record an independent re-observation of a learning: increment corroboration, nudge confidence, reclassify origin. Not an outcome."
    )]
    pub async fn record_corroboration(
        &self,
        Parameters(args): Parameters<RecordCorroborationArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        impls::run(impls::record_corroboration(self, args)).await
    }

    /// Tag and store a learning directly in the knowledge ledger.
    #[tool(
        name = "tag_learning",
        annotations(read_only_hint = false),
        description = "Tag and store a learning directly in the knowledge ledger."
    )]
    pub async fn tag_learning(
        &self,
        Parameters(args): Parameters<TagLearningArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        impls::run(impls::tag_learning(self, args)).await
    }

    /// Record a handoff at a pause-point so the next session can resume.
    #[tool(
        name = "tag_handoff",
        annotations(read_only_hint = false),
        description = "Record a handoff at a pause-point capturing completed/pending tasks, \
                       blockers, modified files, and free-form context notes. Use when the \
                       user pauses work, switches focus, or ends a session — the next \
                       session uses get_handoff to resume."
    )]
    pub async fn tag_handoff(
        &self,
        Parameters(args): Parameters<TagHandoffArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        impls::run(impls::tag_handoff(self, args)).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for CortexServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            server_info: Implementation {
                name: "claude-cortex".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Implementation::default()
            },
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            instructions: Some(if self.read_only {
                "Read-only recall over the cortex ledgers (project of the cwd, then global). \
                 Use search_learnings / get_learning / recall_context."
                    .into()
            } else {
                "Persistent memory for Claude Code. \
                 Use tag_learning to store insights, search_learnings to recall, \
                 record_outcome to reinforce."
                    .into()
            }),
        }
    }
}

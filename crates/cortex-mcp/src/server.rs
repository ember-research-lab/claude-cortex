//! rmcp `ServerHandler` exposing the 12 cortex tools over stdio.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};

use crate::tools::{
    args::{
        EntitySearchArgs, EntityShowArgs, EntityStatsArgs, GetHandoffArgs, GetLearningArgs,
        GetSessionSummaryArgs, GetSuggestionsArgs, LedgerStatsArgs, ListLearningsArgs,
        RecordOutcomeArgs, SearchLearningsArgs, TagHandoffArgs, TagLearningArgs,
    },
    impls,
};

/// Server state shared across tool handlers.
#[derive(Clone)]
pub struct CortexServer {
    /// Optional default project directory. When provided, all `project_dir`
    /// arguments default to this. Mostly useful for tests.
    pub default_project_dir: Option<PathBuf>,
    pub(crate) tool_router: ToolRouter<Self>,
}

impl CortexServer {
    pub fn new() -> Self {
        Self {
            default_project_dir: None,
            tool_router: Self::tool_router(),
        }
    }

    pub fn with_default_project_dir(mut self, dir: PathBuf) -> Self {
        self.default_project_dir = Some(dir);
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

#[tool_router(router = tool_router)]
impl CortexServer {
    /// Search the knowledge ledger using full-text (substring) match.
    #[tool(
        name = "search_learnings",
        description = "Search the knowledge ledger using full-text search."
    )]
    pub async fn search_learnings(
        &self,
        Parameters(args): Parameters<SearchLearningsArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::search_learnings(self, args)).await
    }

    /// Get full details of a specific learning by ID.
    #[tool(
        name = "get_learning",
        description = "Get full details of a specific learning by ID."
    )]
    pub async fn get_learning(
        &self,
        Parameters(args): Parameters<GetLearningArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::get_learning(self, args)).await
    }

    /// Record outcome for a learning (updates confidence via reinforcement).
    #[tool(
        name = "record_outcome",
        description = "Record outcome for a learning (updates confidence via reinforcement)."
    )]
    pub async fn record_outcome(
        &self,
        Parameters(args): Parameters<RecordOutcomeArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::record_outcome(self, args)).await
    }

    /// List learnings from the ledger sorted by confidence.
    #[tool(
        name = "list_learnings",
        description = "List learnings from the ledger sorted by confidence."
    )]
    pub async fn list_learnings(
        &self,
        Parameters(args): Parameters<ListLearningsArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::list_learnings(self, args)).await
    }

    /// Get statistics about the knowledge ledger.
    #[tool(
        name = "ledger_stats",
        description = "Get statistics about the knowledge ledger."
    )]
    pub async fn ledger_stats(
        &self,
        Parameters(args): Parameters<LedgerStatsArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::ledger_stats(self, args)).await
    }

    /// Tag and store a learning directly in the knowledge ledger.
    #[tool(
        name = "tag_learning",
        description = "Tag and store a learning directly in the knowledge ledger."
    )]
    pub async fn tag_learning(
        &self,
        Parameters(args): Parameters<TagLearningArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::tag_learning(self, args)).await
    }

    /// Get recent session summaries derived from ledger blocks.
    #[tool(
        name = "get_session_summary",
        description = "Get recent session summaries for context."
    )]
    pub async fn get_session_summary(
        &self,
        Parameters(args): Parameters<GetSessionSummaryArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::get_session_summary(self, args)).await
    }

    /// Get the latest work-in-progress handoff for session continuity.
    #[tool(
        name = "get_handoff",
        description = "Get the latest work-in-progress handoff for session continuity. \
                       With session_id, returns the latest handoff for that session; \
                       without, returns the most recent handoff across all sessions."
    )]
    pub async fn get_handoff(
        &self,
        Parameters(args): Parameters<GetHandoffArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::get_handoff(self, args)).await
    }

    /// Record a handoff at a pause-point so the next session can resume.
    #[tool(
        name = "tag_handoff",
        description = "Record a handoff at a pause-point capturing completed/pending tasks, \
                       blockers, modified files, and free-form context notes. Use when the \
                       user pauses work, switches focus, or ends a session — the next \
                       session uses get_handoff to resume."
    )]
    pub async fn tag_handoff(
        &self,
        Parameters(args): Parameters<TagHandoffArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::tag_handoff(self, args)).await
    }

    /// Get cross-project learning suggestions from the global ledger.
    #[tool(
        name = "get_suggestions",
        description = "Get cross-project learning suggestions from the global ledger."
    )]
    pub async fn get_suggestions(
        &self,
        Parameters(args): Parameters<GetSuggestionsArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::get_suggestions(self, args)).await
    }

    /// Search for code entities (functions, classes, methods) by name.
    #[tool(
        name = "entity_search",
        description = "Search for code entities (functions, classes, methods) by name."
    )]
    pub async fn entity_search(
        &self,
        Parameters(args): Parameters<EntitySearchArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::entity_search(self, args)).await
    }

    /// Get details of a specific code entity including its relationships.
    #[tool(
        name = "entity_show",
        description = "Get details of a specific code entity including its relationships."
    )]
    pub async fn entity_show(
        &self,
        Parameters(args): Parameters<EntityShowArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::entity_show(self, args)).await
    }

    /// Get statistics about the code entity graph.
    #[tool(
        name = "entity_stats",
        description = "Get statistics about the code entity graph."
    )]
    pub async fn entity_stats(
        &self,
        Parameters(args): Parameters<EntityStatsArgs>,
    ) -> Result<String, ErrorData> {
        impls::run(impls::entity_stats(self, args)).await
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
            instructions: Some(
                "Persistent memory for Claude Code. \
                 Use tag_learning to store insights, search_learnings to recall, \
                 record_outcome to reinforce."
                    .into(),
            ),
        }
    }
}

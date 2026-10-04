//! cortex-mcp: rmcp 0.16 server exposing the cortex ledger tools.
//!
//! 7 read tools (search_learnings, recall_context, get_learning, list_learnings,
//! ledger_stats, get_session_summary, get_handoff), all annotated
//! `readOnlyHint=true`, plus 4 write tools (tag_learning, record_outcome,
//! record_corroboration, tag_handoff) that are registered only when the server
//! is not read-only (`--read-only` / `CORTEX_READ_ONLY=1`). Application errors
//! are returned with `isError=true`.

pub mod paths;
pub mod server;
pub mod tools;

pub use server::CortexServer;

//! cortex-mcp: rmcp 0.16 server exposing the 12 cortex tools.
//!
//! Tool signatures and behavior preserve v2 (per the v3 spec). Internal
//! implementations are simpler — substring search instead of SQLite FTS5,
//! ledger-derived session summaries, etc. The 5 entity/handoff/suggestion
//! tools that depend on subsystems not yet ported return structured
//! "pending" responses; cortex-orientation skill instructs callers on
//! what's available.

pub mod paths;
pub mod server;
pub mod tools;

pub use server::CortexServer;

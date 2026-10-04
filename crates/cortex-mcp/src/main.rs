//! cortex-mcp: stdio MCP server entrypoint.
//!
//! Speaks the MCP wire protocol over stdin/stdout (per the rmcp `stdio()`
//! transport). Claude Code launches this binary as a subprocess; all tool
//! calls flow over the transport.

use clap::Parser;
use cortex_mcp::CortexServer;
use rmcp::ServiceExt;

/// Persistent memory MCP server for Claude Code.
#[derive(Parser, Debug)]
#[command(name = "cortex-mcp", version, about)]
struct Args {
    /// Optional default project directory. If not provided, tools without an
    /// explicit `project_dir` argument fall back to the global ledger.
    #[arg(long)]
    default_project_dir: Option<std::path::PathBuf>,

    /// Register read tools only (no tag_learning / record_outcome /
    /// record_corroboration / tag_handoff). Also enabled by CORTEX_READ_ONLY=1.
    #[arg(long)]
    read_only: bool,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let env_ro =
        cortex_mcp::server::parse_read_only_env(std::env::var("CORTEX_READ_ONLY").ok().as_deref())
            .map_err(|e| anyhow::anyhow!(e))?;
    let mut server = if args.read_only || env_ro {
        CortexServer::new_read_only()
    } else {
        CortexServer::new()
    };
    if let Some(dir) = args.default_project_dir {
        server = server.with_default_project_dir(dir);
    }
    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("rmcp serve: {e}"))?;
    service
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("rmcp waiting: {e}"))?;
    Ok(())
}

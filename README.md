# claude-cortex

Persistent memory that makes Claude Code smarter across sessions.

cortex is a Claude Code plugin providing memory, learning, and continuous-improvement infrastructure. Learnings are recorded in a blockchain-style ledger with hash-chained, Ed25519-signed blocks and BLAKE3 content-addressed storage. Confidence updates with Success / Partial / Failure outcomes and decays on a 180-day half-life so old guidance fades unless reinforced.

This is **v3** — a Rust workspace refactor of the original Python plugin. The on-disk substrate format is preserved exactly, so existing v2 ledgers continue to work.

## Status

v3 lives at `ember-research-lab/claude-cortex` (this repo). v2 (Python) remains at `aaronb305/claude-cortex` for legacy installs; existing v2 ledgers convert via `cortex-migrate`.

| Phase | Scope | Status |
|------|------|------|
| 1 | Cargo workspace + plugin.json + CI | done |
| 2 | `cortex-core` substrate (ledger, hash chain, signatures, Merkle, content store, v2 compat) | done |
| 3 | `cortex-mcp` (rmcp 0.16, 12 tools — 7 ledger-grounded + 5 deferred-to-v3.x stubs) | done |
| 4 | `cortex-hooks` (session_start / post_tool_use / session_end binaries) | done |
| 5 | Skills, agents, commands (markdown) — orientation skill auto-loads | done |
| 6 | `cortex-migrate` (v2 → v3 validation + transcription) | done |
| 7 | Validation (workspace lint + 34-test suite + cold-start benchmarks) | done |
| 8 | Marketplace publishing (`ember-research-lab/marketplace`) | docs ready, awaiting first tag |

**Performance:** hook cold start 3-5 ms (budget: 100 ms). MCP server startup-to-`tools/list` 10-14 ms (budget: 50 ms).

## Workspace layout

```
claude-cortex/
├── Cargo.toml                # Workspace root
├── .claude-plugin/
│   └── plugin.json           # v3 plugin manifest
├── .mcp.json                 # MCP server registration
├── crates/
│   ├── cortex-core/          # Substrate: ledger, hash chain, signing, content store
│   ├── cortex-mcp/           # MCP server (rmcp stdio transport)
│   ├── cortex-hooks/         # session_start, post_tool_use, session_end binaries
│   └── cortex-migrate/       # v2 → v3 ledger validation / import
├── agents/                   # Markdown agent definitions (10)
├── skills/                   # Markdown skill definitions (4)
├── hooks/hooks.json          # SessionStart / PostToolUse / SessionEnd wiring
├── tests/                    # Workspace integration tests + v2 fixtures
└── .github/workflows/        # CI + release pipelines
```

`agents/`, `skills/`, and `commands/` stay markdown — they are dispatched by Claude Code itself and remain language-agnostic across cortex versions.

## Local development

Requires Rust 1.85+ (stable).

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Hook subprocess cold-start budget is **under 100 ms**; MCP server response budget is **under 50 ms** for typical operations. Benchmark before declaring a phase complete.

## Install

Once v0.3.0 ships on the Ember marketplace:

```sh
/plugin marketplace add ember-research-lab/marketplace
/plugin install claude-cortex@ember-research-lab
```

The plugin install fetches the markdown (agents, skills, hooks/hooks.json) and `plugin.json`. It does **not** install the Rust binaries — those need to be on PATH. Two ways:

**A. From a release artifact (recommended):**
```sh
# Download the platform-matching tarball from
# https://github.com/ember-research-lab/claude-cortex/releases
tar -xzf claude-cortex-x86_64-unknown-linux-gnu.tar.gz
cp claude-cortex-*/cortex-* ~/.local/bin/   # or anywhere on PATH
```

**B. From source (Rust ≥ 1.85):**
```sh
git clone https://github.com/ember-research-lab/claude-cortex
cd claude-cortex
cargo install --path crates/cortex-mcp --bins
cargo install --path crates/cortex-hooks --bins
cargo install --path crates/cortex-migrate --bins
```

After install, verify:
```sh
cortex-mcp --version
which cortex-session-start cortex-post-tool-use cortex-session-end cortex-migrate
```

## Migration from v2

Existing v2 users:

1. `/plugin uninstall claude-cortex@aaronb305` (Python v2)
2. `/plugin install claude-cortex@ember-research-lab` (Rust v3)
3. `cortex-migrate --check` once to validate the existing ledger against v3

The on-disk format is identical between v2 and v3, so step 3 is typically a no-op (validation only). Hash chain, signatures, and confidence values all carry over unchanged.

## License

MIT — see [LICENSE](LICENSE).

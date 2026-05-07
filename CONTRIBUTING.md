# Contributing to claude-cortex

## Workspace layout

```
claude-cortex/
├── Cargo.toml                # Workspace root + pinned dep versions
├── crates/
│   ├── cortex-core/          # Substrate: ledger, hash chain, signing, content store, Merkle, v2 compat
│   ├── cortex-mcp/           # MCP server (rmcp 0.16, 12 tools)
│   ├── cortex-hooks/         # session_start, post_tool_use, session_end binaries
│   └── cortex-migrate/       # v2 → v3 ledger validation + transcription
├── agents/                   # Markdown agent definitions (10 agents)
├── skills/                   # Markdown skill definitions (4 skills)
├── commands/                 # Markdown slash command definitions
├── tests/fixtures/v2_ledger/ # Real v2-format ledger for regression tests
└── .github/workflows/        # CI + multi-platform release
```

`cortex-core` is the inner crate; the others depend on it. `cortex-core::v2_compat` is the only place that knows about the v2 wire format — keep it that way.

## Required Rust toolchain

Stable, ≥ 1.85. Workspace pins versions; do not bump individual crates without bumping the `[workspace.dependencies]` line.

## Local development

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Hook subprocess binaries should remain under **100 ms cold start**; the MCP server should respond to typical operations under **50 ms**. Benchmark before declaring a phase complete:

```sh
cargo build --release --bin cortex-mcp
echo '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"bench","version":"0.0.0"}}}' | timeout 2 target/release/cortex-mcp
```

## Adding a new MCP tool

1. Add the argument struct to `crates/cortex-mcp/src/tools/args.rs` — derive `Serialize, Deserialize, JsonSchema`. Field shapes should mirror the Python signature if porting from v2.
2. Add an async impl to `crates/cortex-mcp/src/tools/impls.rs` taking `&CortexServer` + the args struct, returning `anyhow::Result<serde_json::Value>`.
3. Add the `#[tool(name = "...", description = "...")]` declaration on `CortexServer` in `crates/cortex-mcp/src/server.rs`. Wrap the impl call with `impls::run(impls::your_tool(...))` so application errors surface as JSON `{"error": ...}`.
4. Add an integration test to `crates/cortex-mcp/tests/tools.rs` exercising the impl directly.
5. Update `.claude-plugin/plugin.json`'s implicit tool list (the MCP server enumerates them automatically; nothing to add) and update README.

## Adding a new hook

1. Create `crates/cortex-hooks/src/bin/<name>.rs` — read JSON via `cortex_hooks::read_input()`, build the directive string, call `cortex_hooks::write_output(event_name, context)`.
2. Register the binary in `crates/cortex-hooks/Cargo.toml` `[[bin]]` block.
3. Add a corresponding entry to `.claude-plugin/plugin.json`'s hooks configuration.
4. Add an integration test in `crates/cortex-hooks/tests/hooks.rs` that spawns the binary and asserts the JSON output shape.

## Substrate inviolability

The on-disk format of v3 ledgers (block JSON, index.json, reinforcements.json, merkle.json, identity.json, .private_key, trusted_keys.json) is a **public interface**. Changing it is a major-version bump. v4 will read v3 ledgers without migration; respect that.

## Tests

- `cortex-core` unit tests cover hashing, signing, merkle, confidence, time, models — keep these fast (`cargo test -p cortex-core` should be < 1 s).
- `cortex-core/tests/v2_fixtures.rs` exercises the v2 compatibility module against `tests/fixtures/v2_ledger/`. The fixture is generated from real v2 Python code; if you need to regenerate it, see `tests/fixtures/README.md` (TODO).
- `cortex-mcp/tests/tools.rs` exercises tool impls directly — fast (no rmcp wire setup).
- `cortex-hooks/tests/hooks.rs` and `cortex-migrate/tests/migrate.rs` spawn release binaries; require `cargo build` first.

## Release process

Tag `v0.x.y` on `main`. The `.github/workflows/release.yml` matrix builds for linux x86_64 + aarch64, macOS x86_64 + aarch64, windows x86_64; uploads tarballs/zips with SHA-256 sidecars; publishes a GitHub Release. Marketplace pickup is automatic — `ember-research-lab/marketplace` references this repo by `version`.

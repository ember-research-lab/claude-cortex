# claude-cortex

Persistent memory that makes Claude Code smarter across sessions.

cortex is a Claude Code plugin providing memory, learning, and continuous-improvement infrastructure. Learnings are recorded in a blockchain-style ledger with hash-chained, Ed25519-signed blocks and BLAKE3 content-addressed storage. Confidence updates with Success / Partial / Failure outcomes and decays on a 180-day half-life so old guidance fades unless reinforced.

This is **v4** — a spectral-retrieval and handoff-substrate layer on top of the v3 Rust workspace. The on-disk substrate format is preserved exactly across both versions, so existing v2 ledgers continue to work.

## Status

cortex lives at `ember-research-lab/claude-cortex` (this repo). v2 (Python) remains at `aaronb305/claude-cortex` for legacy installs; existing v2 ledgers convert via `cortex-migrate`.

| Phase | Scope | Status |
|------|------|------|
| v3.1 | Cargo workspace + plugin.json + CI | done |
| v3.2 | `cortex-core` substrate (ledger, hash chain, signatures, Merkle, content store, v2 compat) | done |
| v3.3 | `cortex-mcp` (rmcp 0.16, 12 tools — 7 ledger-grounded + handoff + 4 deferred entity-graph stubs) | done |
| v3.4 | `cortex-hooks` (session_start / post_tool_use / session_end binaries) | done |
| v3.5 | Skills, agents, commands (markdown) — orientation injects at SessionStart | done |
| v3.6 | `cortex-migrate` (v2 → v3 validation + transcription) | done |
| v4.1 | `cortex-similarity` (BM25, no embedding model / no API) | done |
| v4.2 | `cortex-spectral` (graph + Laplacian + eigendecomposition) | done |
| v4.3 | `cortex-active-memory` (top-k snapshots with mode projections) | done |
| v4.4 | `cortex-monitor` + `cortex-dream` (spectrum history + trajectory classification) | done |
| v4.5 | `cortex-handoff` (work-in-progress state capture, separate from the long-term ledger) | done |
| v4.6 | Hook token optimization (compressed directive, result-aware skip, dedup window) | done |

**Performance:** hook cold start 3-5 ms (budget: 100 ms). MCP server startup-to-`tools/list` 10-14 ms (budget: 50 ms). cortex-dream pipeline under 60 s for ledgers <10 k entries.

## Workspace layout

```
claude-cortex/
├── Cargo.toml                # Workspace root
├── .claude-plugin/
│   └── plugin.json           # plugin manifest
├── .mcp.json                 # MCP server registration
├── crates/
│   ├── cortex-core/          # Substrate: ledger, hash chain, signing, content store
│   ├── cortex-mcp/           # MCP server (rmcp stdio transport)
│   ├── cortex-hooks/         # session_start, post_tool_use, session_end binaries
│   ├── cortex-migrate/       # v2 → v3 ledger validation / import
│   ├── cortex-similarity/    # v4 BM25 lexical similarity
│   ├── cortex-spectral/      # v4 graph + Laplacian + eigendecomposition
│   ├── cortex-active-memory/ # v4 top-k snapshots with mode projections
│   ├── cortex-monitor/       # v4 spectrum history + trajectory classifier
│   ├── cortex-dream/         # v4 dreaming pipeline orchestrator
│   └── cortex-handoff/       # v4 work-in-progress state substrate
├── agents/                   # Markdown agent definitions (10)
├── skills/                   # Markdown skill definitions (4)
├── commands/                 # Slash commands (/handoff, /cortex-dream)
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

The plugin install fetches the markdown (agents, skills, hooks/hooks.json) and `plugin.json`, plus the `bin/` shims that keep the hooks from hard-failing. It does **not** build the Rust binaries the hooks and MCP server actually run — those need to be on PATH. Until you build them, `cortex-session-start` prints an install reminder and the other hooks no-op; nothing crashes.

**Run the installer once (Rust ≥ 1.85):**
```sh
bash "$CLAUDE_PLUGIN_ROOT/install.sh"   # builds release binaries -> ~/.local/bin
```

`$CLAUDE_PLUGIN_ROOT` points at the installed plugin (under `~/.claude/plugins/cache/...`); from a source checkout just run `bash install.sh` in the repo root. Override the destination with `CORTEX_BIN_DIR=~/bin bash install.sh`. Then **restart Claude Code**.

Alternatives if you prefer to manage binaries yourself:

```sh
# From a release artifact:
tar -xzf claude-cortex-x86_64-unknown-linux-gnu.tar.gz
cp claude-cortex-*/cortex-* ~/.local/bin/   # or anywhere on PATH

# Or per-crate from source:
cargo install --path crates/cortex-mcp --bins
cargo install --path crates/cortex-hooks --bins
cargo install --path crates/cortex-migrate --bins
```

After install, verify:
```sh
cortex-mcp --version
which cortex-session-start cortex-post-tool-use cortex-session-end cortex-migrate
```

## Upgrading

> **Important:** plugin updates do NOT update the binaries.

Claude Code's plugin loader refreshes the markdown / hooks.json / plugin.json / `bin/` shims via `git pull` on plugin reload, but it does not rebuild the Rust binaries on PATH. After every cortex release, refresh the binaries explicitly — just re-run the installer:

```sh
bash "$CLAUDE_PLUGIN_ROOT/install.sh"
```

Or manage them yourself:
```sh
# From the release artifact
curl -L https://github.com/ember-research-lab/claude-cortex/releases/latest/download/claude-cortex-x86_64-unknown-linux-gnu.tar.gz \
  | tar -xz
cp claude-cortex-*/cortex-* ~/.local/bin/

# Or per-crate from a fresh source tree
cd /path/to/claude-cortex && git pull
cargo install --path crates/cortex-mcp --bins
cargo install --path crates/cortex-hooks --bins
cargo install --path crates/cortex-migrate --bins
```

Then **restart Claude Code** so existing sessions pick up the new binaries. Symptom of forgetting this step: source-side features (e.g. new skill content, updated hook directives) appear to work, but functionality that lives in the binary (e.g. SessionStart auto-injection, new MCP tools) is silently absent.

## Migration from v2

Existing v2 users:

1. `/plugin uninstall claude-cortex@aaronb305` (Python v2)
2. `/plugin install claude-cortex@ember-research-lab` (Rust v3)
3. **Run `cortex-migrate` against your ledger** — see below. This is a real conversion, not a no-op.
4. **Restart Claude Code** so the new MCP server reads the migrated ledger.

The on-disk format is **not** identical between v2 and v3, despite carrying the same filenames:

- `reinforcements.json` — v2 has a `privacy` field and naive timestamps (no `Z`); v3 requires `last_applied`, `block_id`, `content_hash`, `object_store_hash` and rejects naive timestamps via strict RFC3339 deserialize.
- `index.json` / `blocks/*.json` — v2 timestamps are written by Pydantic with `Z` but hashed via Python `json.dumps(sort_keys=True)` against `+00:00` form; v3 hashes against `Z` form via serde-json. So v2 stored hashes don't match v3 recomputation; `cortex-migrate` re-hashes during transcription.

**How to migrate (recommended flow):**

```sh
# 1. Sanity check — read-only validation against v2 hash form
cortex-migrate --from ~/.claude/ledger --check

# 2. Migrate to a scratch dir; --force tolerates the hash mismatches above
cortex-migrate --from ~/.claude/ledger --to /tmp/ledger-v3 --force

# 3. Preserve any non-ledger state the migrator doesn't touch
cp -r ~/.claude/ledger/cortex-state /tmp/ledger-v3/
cp ~/.claude/ledger/imports.json /tmp/ledger-v3/ 2>/dev/null || true

# 4. Atomic swap (do this with no Claude Code session running)
mv ~/.claude/ledger ~/.claude/ledger.preswap-$(date +%F)
mv /tmp/ledger-v3 ~/.claude/ledger
```

Audit trail of re-hashed blocks lives in `~/.claude/ledger/MIGRATION.json` after step 4.

**Symptom of skipping migration:** `tag_learning` returns `json decode at .../reinforcements.json: premature end of input` (or, on a partially-touched ledger, `missing field 'last_applied'`). The runtime does not auto-migrate — it expects v3-shape on disk.

**Hybrid state warning:** if you ever ran a v3 binary against an un-migrated ledger, your `index.json` may already be v3-shape while `reinforcements.json` is still v2-shape. The `--force` flag handles this — the migrator re-hashes any v3-form blocks it finds and transcribes the v2 reinforcements.

## License

MIT — see [LICENSE](LICENSE).

# CLAUDE.md — claude-cortex

Orientation for anyone (human or Claude) working in this repo. Read this first; it's the context a fresh session doesn't have.

## What this is

**Persistent memory that makes Claude Code smarter across sessions.** cortex is a Claude Code *plugin* (markdown agents/skills/commands + hooks + an MCP server) backed by a **Rust workspace**. Learnings are recorded in a blockchain-style **ledger**: hash-chained, Ed25519-signed blocks with a content-addressed object store and a Merkle root. Confidence updates on Success / Partial / Failure outcomes and decays on a 180-day half-life so stale guidance fades unless reinforced.

This is the **v3+ Rust rewrite** (v2 was Python, still at `aaronb305/claude-cortex` for legacy installs). The v4 layer adds spectral retrieval + handoff substrate, v5 adds episodic consolidation. **The on-disk substrate format is preserved from v2** — see "What not to touch."

**Read before substantial work:**
- [`README.md`](README.md) — install/upgrade/migration flows, phase status, performance budgets.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — crate map, how to add an MCP tool / hook, substrate inviolability, release process.
- [`docs/v4-plan-of-record.md`](docs/v4-plan-of-record.md) and [`docs/episodic-consolidation-spec.md`](docs/episodic-consolidation-spec.md) — design intent for the v4/v5 layers.

## Architecture: crates → roles

```
crates/cortex-core/          Substrate. Ledger, hash chain, Ed25519 signing, content store,
                             Merkle, confidence/decay, v2_compat. The inner crate; all others depend on it.
crates/cortex-mcp/           MCP server (rmcp 0.16, stdio). The `cortex` tool surface Claude calls.
crates/cortex-hooks/         session_start / post_tool_use / session_end / pre_compact binaries.
crates/cortex-migrate/       v2 → v3 ledger validation + transcription (re-hashes blocks).
crates/cortex-similarity/    v4 BM25 lexical similarity (no embedding model, no API).
crates/cortex-spectral/      v4 graph + Laplacian + eigendecomposition (nalgebra).
crates/cortex-active-memory/ v4 top-k snapshots with mode projections.
crates/cortex-monitor/       v4 spectrum history + trajectory classification.
crates/cortex-dream/         v4 dreaming pipeline orchestrator (index → graph → decompose → snapshot).
crates/cortex-handoff/       v4 work-in-progress state substrate (ephemeral; NOT the long-term ledger).
crates/cortex-episodic/      v5 episode capture + lazy outcome-gated eviction + consolidation.
```

Plugin assets are **markdown and stay markdown** — Claude Code dispatches them, so they are language-agnostic across cortex versions:
- `agents/` (10 agents, listed in `plugin.json`), `skills/` (cortex-orientation, handoff-management, learning-capture, ledger-knowledge), `commands/` (`/cortex-dream`, `/handoff`).
- `hooks/hooks.json` wires SessionStart / PostToolUse / SessionEnd / PreCompact to the `cortex-*` binaries; `bin/` holds graceful-degradation shims used until the real binaries are built.

## The substrate / ledger — and why its format is inviolable

The on-disk v3 ledger is a **public interface**: block JSON, `index.json`, `reinforcements.json`, `merkle.json`, `identity.json`, `.private_key`, `trusted_keys.json`. A new cortex version must read an existing ledger **without migration**; changing the on-disk shape is a **major-version bump**, not a casual edit. **Never break substrate compatibility.** The v2 wire format is known in exactly one place — `cortex-core::v2_compat` — keep it that way.

Hashing note (verified, easy to get wrong): despite the README/spec mentioning BLAKE3, **all hashes are SHA-256** (`cortex-core/src/lib.rs` documents this — every other piece of tooling expects SHA-256). Timestamps are strict RFC3339 `Z` form.

## House rules

1. **`cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings` must be clean.** CI sets `RUSTFLAGS: -D warnings`; warnings fail the build.
2. **Tests run on three OSes** — CI matrix is ubuntu / macos / windows (`cargo build` + `cargo test --workspace`). Don't assume Linux-only behavior (paths, line endings).
3. **`cargo-deny check advisories bans sources` must pass** (`deny.toml`). No unknown registries/git sources; new advisories need an explicit, reasoned ignore.
4. **Every workspace crate is `publish = false`** — these are not published to crates.io; they ship as built binaries via the marketplace/release pipeline.
5. **Pin deps at the workspace root.** Versions live in `[workspace.dependencies]` in the root `Cargo.toml`; don't bump a single crate's dep without bumping the workspace line.
6. **Performance budgets are real:** hook cold start < 100 ms, MCP typical response < 50 ms. Benchmark before declaring a phase done.
7. **Commit format:** `type(scope): description — detail`, conventional types (`feat`, `fix`, `docs`, `chore`, `ci`, `test`, `refactor`). Scope is usually the crate (e.g. `feat(episodic): …`, `fix(install): …`).
8. **Bump `plugin.json` `version` on every release** — Claude Code keys plugin updates on it; a `version-guard` CI job blocks a release tag whose `plugin.json` version mismatches.

## Commands

```sh
cargo build  --workspace
cargo test   --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt    --all
cargo deny   check advisories bans sources         # supply-chain gate

bash install.sh                                    # build release binaries -> ~/.local/bin (Rust >= 1.85)
cargo build --release --bin cortex-mcp             # for the MCP startup benchmark in CONTRIBUTING.md
```

Requires Rust stable ≥ 1.85.

## What NOT to touch casually

- **The substrate format** (on-disk ledger files above) — breaking it breaks every existing user's memory. Major-version change only; v2 logic lives only in `cortex-core::v2_compat`.
- **Hash-chain / signing / Merkle / integrity code** (`cortex-core::{hashing,signing,merkle,store}`) — these define block identity and the chain's verifiability. A wrong hash form silently invalidates ledgers and breaks v2 round-trips. **Crypto/integrity changes need careful review** and v2-fixture coverage (`cortex-core/tests/v2_fixtures.rs` against `tests/fixtures/v2_ledger/`).
- **The keys/identity files** (`.private_key`, `trusted_keys.json`, `identity.json`) — signing material; don't commit real ones or change their schema lightly.

## Plugin packaging

- `.claude-plugin/plugin.json` — manifest: declares the 10 agents and registers the `cortex` **MCP server** (`command: "cortex-mcp"`).
- `.mcp.json` — same MCP-server registration for direct (non-plugin) use.
- Install pulls markdown + `plugin.json` + `bin/` shims, but does **not** build the Rust binaries — `install.sh` (or `cargo install --path crates/cortex-mcp|cortex-hooks|cortex-migrate --bins`) does, onto PATH. Binaries are never auto-updated on plugin upgrade; re-run the installer and restart Claude Code. See README "Upgrading."

## Adding things (see CONTRIBUTING.md for the full recipe)

- **New MCP tool:** args struct in `cortex-mcp/src/tools/args.rs` → impl in `tools/impls.rs` → `#[tool(...)]` decl in `server.rs` → integration test in `tests/tools.rs`.
- **New hook:** binary in `cortex-hooks/src/bin/<name>.rs` → register `[[bin]]` in its `Cargo.toml` → entry in `plugin.json`/`hooks.json` → spawn-and-assert test in `tests/hooks.rs`.

Only the above is stated from what's verifiable in the repo; when in doubt, check the source before relying on a claim.

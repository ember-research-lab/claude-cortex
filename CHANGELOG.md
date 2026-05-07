# Changelog

All notable changes to claude-cortex are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.3.1] — 2026-05-07

Patch driven by self-introspection during the first real session of v0.3.0.

### Fixed
- **Skills now load.** All four cortex skills (`cortex-orientation`, `ledger-knowledge`, `learning-capture`, `handoff-management`) had `allowed-tools:` in their YAML frontmatter, which is a non-standard field that fails silent validation in Claude Code's plugin loader. Working SKILL.md files use only `name`, `description`, `version`, `license`. Removed `allowed-tools` and added `version: 0.3.1`. Without this fix, none of cortex's skills surfaced as available — most importantly, `cortex-orientation` (which is supposed to auto-load every session and establish orchestrator-identity directives) was completely dark.
- **`post_tool_use` hook no longer over-fires.** v0.3.0 used a denylist of routine tools (Read/Glob/Grep/Task*) and fired on everything else, which meant Bash, Write, Edit, ToolSearch, and even cortex's own MCP tools all triggered a discovery-tagging directive. Worse, calling `tag_learning` itself fired the hook, which prompted a tag for the tag, recursing on its own output. Inverted to an *allowlist*: now fires only on `WebFetch`, `WebSearch`, and external MCP tools. Cortex's own MCP tools are explicitly excluded to prevent the recursion.

### Migration
No data migration needed — these are config-and-prompt-only changes. Pull the new release; `/plugin marketplace update ember-research-lab` and `/reload-plugins`.

## [0.3.0] — 2026-05-06

Major-version refactor: Python → Rust workspace, `aaronb305/claude-cortex` → `ember-research-lab/claude-cortex`, adopting the v3 spec's design principles (orchestrator identity, situation modeling, producer/verifier separation, substrate inviolability).

### Added
- Rust workspace (`cortex-core`, `cortex-mcp`, `cortex-hooks`, `cortex-migrate`).
- v3-native ledger format: RFC3339 `Z` timestamps, canonical sorted-key JSON for block hashes, SHA-256 throughout (matches v2 hash algorithm; only the timestamp/JSON canonicalization differs).
- `cortex-core::v2_compat` module for read-only access to v2 ledgers, including Python-compatible canonical JSON serializer used to recompute v2 block hashes byte-for-byte before migration.
- `cortex-migrate` binary for one-shot v2 → v3 ledger transcription. Validates v2 hashes before writing; idempotent re-run; emits `MIGRATION.json` audit trail.
- Four new agents: `verifier` (DERIVED/PARTIAL/CANNOT_DERIVE), `falsifier-spec`, `task-decomposer`, `outcome-recorder`.
- `cortex-orientation` SKILL — auto-loaded every session, establishes orchestrator identity + 6 design principles.
- Multi-platform release workflow: linux x86_64/aarch64, macOS x86_64/aarch64, windows x86_64 with SHA-256 sidecars.
- Marketplace template at `.claude-plugin/marketplace.json` for `ember-research-lab/marketplace` repo.

### Changed
- All hooks rewritten as Rust binaries: cold-start 3-5 ms (was Python 50-200 ms).
- MCP server rewritten using `rmcp 0.16`: stdio response budget 50 ms, observed 10-14 ms.
- Project ledger location moved from `<project>/.claude/ledger/` to `<project>/.claude/cortex/ledger/` (cache directory now reserved for ephemeral state only).
- Tool signatures preserved; tool *implementations* simplified (substring search instead of SQLite FTS5; ledger-derived session summaries).
- `continuous-runner` agent retired; default operating mode lives in `cortex-orientation` skill instead.

### Removed
- `[DISCOVERY]/[DECISION]/[ERROR]/[PATTERN]` inline tag pattern. Capture via `tag_learning` MCP tool instead.
- Plugin-root `CLAUDE.md` (it was never loaded by Claude Code anyway). Behavioral directives moved into the orientation skill.

### Deferred to v3.x point releases
- `get_handoff` (handoff store substrate)
- `get_suggestions` (cross-project recommender)
- `entity_search` / `entity_show` / `entity_stats` (tree-sitter entity index)

These tools ship with their v2-compatible signatures but return structured "feature pending v3.x port" responses. v4's spectral retrieval (cortex-spectral crate) is expected to subsume most of this surface area.

### Pre-release fixes (post plugin-validator review)
- Hooks now declared in `hooks/hooks.json` (auto-discovered) so SessionStart / PostToolUse / SessionEnd actually fire after install. Previously the binaries existed but were never wired into Claude Code's hook system.
- `marketplace.json` source schema fixed (`source.type` discriminator instead of `source.source`); marketplace install would have failed without this.
- `mcpServers` inlined in `plugin.json` instead of pointing at a sibling path; matches the canonical plugin loader expectation.
- Empty `commands/` directory removed (no slash commands ship in v0.3.0).

### Migration from v2
1. `/plugin uninstall claude-cortex@aaronb305` (Python v2)
2. `/plugin install claude-cortex@ember-research-lab` (Rust v3)
3. `cortex-migrate --from <project>/.claude/ledger --to <project>/.claude/cortex/ledger`
   The transcription validates every v2 hash and recomputes the merkle root before writing; existing identity/private key/trusted keys are copied across so the chain of custody is preserved.

[Unreleased]: https://github.com/ember-research-lab/claude-cortex/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/ember-research-lab/claude-cortex/releases/tag/v0.3.0

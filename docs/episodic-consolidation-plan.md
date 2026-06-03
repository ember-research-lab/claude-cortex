# cortex — Episodic Consolidation: Implementation Plan

Grounded in the actual codebase as of 2026-06-02. Companion to
`docs/episodic-consolidation-spec.md`. Prerequisites: spec read in full;
codebase read (`cortex-handoff/src/lib.rs`, `cortex-core/src/{objects,store}.rs`,
`cortex-hooks/src/{lib,bin/*}`, `cortex-dream/src/lib.rs`, `hooks/hooks.json`,
`agents/outcome-recorder.md`).

---

## Shared-helpers refactor (pre-Phase-1)

The spec requires factoring duplicated dir+pointer and atomic-write helpers
from `cortex-handoff` into `cortex-core` before building `cortex-episodic`.

**What exists today:**

- `cortex-core/src/objects.rs` already has `write_atomic_json` (pub(crate),
  re-exported as `_write_atomic_json` from `store.rs` — private accident, not
  intentional API).
- `cortex-handoff/src/lib.rs` duplicates the temp+rename pattern in
  `record_handoff` rather than calling `cortex-core`'s version (it can't — it's
  `pub(crate)`).
- The current/pointer file pattern (write filename → temp+rename) appears only
  in `cortex-handoff`; `cortex-active-memory` likely has a copy (not yet
  implemented beyond scaffolding, but the v4 dream pipeline writes `current`
  there too).

**What to move into `cortex-core`:**

Add `cortex-core/src/persist.rs` exposing:

```rust
pub fn write_atomic_json<T: serde::Serialize>(target: &Path, value: &T) -> Result<()>;
pub fn write_pointer(dir: &Path, pointer_name: &str, filename: &str) -> Result<()>;
pub fn read_pointer(dir: &Path, pointer_name: &str) -> Result<Option<String>>;
```

`write_atomic_json` is a promotion of the existing `pub(crate)` version in
`objects.rs` (no logic change, just visibility). `write_pointer` / `read_pointer`
are extracted from `cortex-handoff::record_handoff` / `read_current`.

Re-export from `cortex-core/src/lib.rs`:
```rust
pub use persist::{write_atomic_json, write_pointer, read_pointer};
```

Update `cortex-handoff/src/lib.rs` to call `cortex_core::write_atomic_json`
and `cortex_core::write_pointer` / `read_pointer`; delete the local copies.

This is a pure refactor: no behaviour change, all existing
`cortex-handoff` tests continue to pass, no new workspace dependencies.

---

## Phase 1 — `cortex-episodic` crate: types + capture + eviction

### Files to create

```
crates/cortex-episodic/
├── Cargo.toml
└── src/
    ├── lib.rs          # pub re-exports; module declarations
    ├── manifest.rs     # EpisodeManifest, EpisodeStatus, load/save manifest
    ├── episode.rs      # EpisodeRecord type + constructors
    ├── capture.rs      # capture_tail(), watermark helpers
    └── eviction.rs     # reconcile_eviction() — pure function of manifest + ledger
```

**Workspace change:** add `"crates/cortex-episodic"` to `[workspace] members`
in `Cargo.toml`. Add `cortex-episodic = { path = "crates/cortex-episodic" }` to
`[workspace.dependencies]`.

**`Cargo.toml` for the crate:**
```toml
[package]
name = "cortex-episodic"
# workspace.package fields...

[dependencies]
cortex-core.workspace = true
serde.workspace = true
serde_json.workspace = true
anyhow.workspace = true
chrono.workspace = true
uuid.workspace = true
```

### Key types (from spec, grounded in existing model conventions)

**`episode.rs`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeStatus {
    Unconsolidated,
    Consolidating,
    ConsolidatedPendingConfirmation,
    Evictable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeRecord {
    pub episode_id: String,           // Uuid::new_v4()
    pub session_id: String,
    pub captured_at: UtcTime,         // cortex_core::time::UtcTime
    pub capture_source: String,       // "precompact:auto" | "precompact:manual" | "sessionend:{reason}"
    pub transcript_path: Option<String>,
    pub byte_range: [u64; 2],         // [start_offset, end_offset]
    pub status: EpisodeStatus,
    #[serde(default)]
    pub promoted_block_ids: Vec<String>,
    pub custom_instructions: Option<String>,
}

impl EpisodeRecord {
    pub fn new(session_id: impl Into<String>, capture_source: impl Into<String>,
               transcript_path: Option<String>, byte_range: [u64; 2]) -> Self { ... }

    pub fn filename(&self) -> String {
        // "episode-{session_id}-{rfc3339-z-safe}.json"
        // Uses cortex_core::time::format_rfc3339_z + ':' → '-' replace,
        // matching the handoff filename convention in cortex-handoff.
    }
}
```

**`manifest.rs`**

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpisodeManifest {
    // Key: session_id. Value: last captured byte offset.
    pub consolidated_through_offsets: BTreeMap<String, u64>,
    // All known episodes keyed by episode_id.
    pub episodes: BTreeMap<String, EpisodeRecord>,
}

pub fn episodic_dir(state_root: &Path) -> PathBuf { state_root.join("episodic") }
pub fn manifest_path(state_root: &Path) -> PathBuf { episodic_dir(state_root).join("manifest.json") }

pub fn load_manifest(state_root: &Path) -> anyhow::Result<EpisodeManifest> { ... }
// Uses cortex_core::write_atomic_json (after the shared-helpers refactor).
pub fn save_manifest(state_root: &Path, m: &EpisodeManifest) -> anyhow::Result<()> { ... }
```

**`capture.rs`**

```rust
/// Read the transcript from `transcript_path` (if present), compute the new
/// tail starting at `watermark` bytes, write the episode JSON, and advance
/// the watermark in the manifest. Returns None if the file has not grown past
/// the watermark (idempotent no-op).
pub fn capture_tail(
    state_root: &Path,
    session_id: &str,
    transcript_path: Option<&str>,
    capture_source: &str,
    custom_instructions: Option<&str>,
) -> anyhow::Result<Option<EpisodeRecord>> { ... }
```

Note: transcript byte-range captures the actual file offsets. The function must
handle the case where `transcript_path` is None or the file is absent (common
in tests or when Claude Code's `transcript_path` field is unset) — in that case
write the episode with `byte_range: [0, 0]` rather than failing.

**`eviction.rs`**

```rust
/// Pure: given the current manifest and the ledger's reinforcements,
/// mark episodes as Evictable when all their promoted_block_ids have
/// a positive record_outcome OR are older than TTL_DAYS without contradiction.
/// Returns the updated manifest (does NOT write it; caller persists).
pub fn reconcile_eviction(
    manifest: EpisodeManifest,
    reinforcements: &cortex_core::models::Reinforcements,
    ttl_days: u32,
) -> EpisodeManifest { ... }

/// Prune episode files for all Evictable episodes; advance manifest.
pub fn prune_evictable(state_root: &Path, manifest: &mut EpisodeManifest) -> anyhow::Result<usize> { ... }
```

`reconcile_eviction` is a pure function so it can be tested without I/O.
It checks `promoted_block_ids` against `reinforcements.learnings`:
- A block_id confirms when any of its outcomes has `result == Success` (or
  Partial with enough count).
- TTL backstop: `captured_at` + `ttl_days` < now → evictable regardless.

### Phase 1 tests (gate: must pass before Phase 2)

File: `crates/cortex-episodic/tests/phase1.rs`

```rust
// round_trip_capture_writes_correct_byte_range
// Fixture: write a 200-byte temp file. Call capture_tail with watermark=0.
// Assert episode byte_range == [0, 200], episode_id is non-empty,
// manifest watermark == 200, episode file exists on disk.

// second_capture_at_same_watermark_is_noop
// After the first capture above, call capture_tail again with same watermark.
// Assert returns None, manifest.episodes still has exactly 1 entry,
// no new file written.

// capture_tail_advances_watermark_on_append
// After first capture, append 50 more bytes to the file.
// Call capture_tail again. Assert returns Some with byte_range [200, 250],
// manifest.episodes has 2 entries.

// capture_without_transcript_path_writes_zero_range
// Call capture_tail with transcript_path=None.
// Assert returns Some with byte_range [0, 0], does not error.

// reconcile_eviction_pure_function
// Build a manifest with 2 episodes: one with promoted_block_ids = ["block-A"],
// one with promoted_block_ids = ["block-B"]. Build a Reinforcements with:
// - "block-A" has a Success outcome (enough to confirm).
// - "block-B" has no outcomes, captured_at 35 days ago (within 30-day TTL).
// Assert reconcile_eviction marks block-A's episode Evictable, block-B remains
// ConsolidatedPendingConfirmation.

// reconcile_eviction_ttl_backstop
// Same setup but captured_at 31 days ago for block-B with TTL=30.
// Assert both episodes become Evictable.

// eviction_never_prunes_unconsolidated
// Episode with status=Unconsolidated, TTL exceeded.
// Assert reconcile_eviction leaves it Unconsolidated (only
// ConsolidatedPendingConfirmation is eligible).
```

Fixtures needed: a seeded `TempDir` with a fake transcript text file; a
`Reinforcements` built directly (no real ledger needed — just struct
construction matching `cortex-core::models::Reinforcements`).

---

## Phase 2 — Capture hooks

### Files to create/modify

**New binary:**
```
crates/cortex-hooks/src/bin/pre_compact.rs
```

**Modify:**
```
crates/cortex-hooks/src/bin/session_end.rs   # add capture_tail call
crates/cortex-hooks/Cargo.toml               # add cortex-episodic dep + new [[bin]]
hooks/hooks.json                             # register PreCompact
bin/cortex-pre-compact                       # new shim (mirrors cortex-session-start shim)
install.sh                                   # add cortex-pre-compact to BINS
```

### `pre_compact.rs`

```rust
// Reads HookInput (session_id, transcript_path, extra.trigger: "auto"|"manual",
// extra.custom_instructions: Option<String>).
// Derives state_root from cwd the same way session_start.rs derives ledger path
// (project_dir() from cortex_hooks::project_dir, then join "cortex-state").
// Calls cortex_episodic::capture_tail(...).
// On success: exit 0, empty stdout (PreCompact hook output is not read back
// as additionalContext; the hook is async/non-blocking so we do not emit JSON).
// On error: eprintln! to stderr (non-fatal), exit 0 anyway — must not block
// compaction.
```

The `trigger` field from `extra` maps to `capture_source`:
- `trigger == "auto"` → `"precompact:auto"`
- `trigger == "manual"` → `"precompact:manual"`
- absent / unknown → `"precompact:auto"` (safe default)

**`hooks/hooks.json` addition:**

```json
"PreCompact": [
  {
    "hooks": [
      {
        "type": "command",
        "command": "cortex-pre-compact",
        "background": true
      }
    ]
  }
]
```

`"background": true` is the async/non-blocking declaration per Claude Code hook
docs (spec requirement: "async, non-blocking"). Verify against Claude Code hook
docs at implementation time — the exact field name may differ.

**`session_end.rs` extension:**

Add a `capture_tail` call after the existing directive print, using
`capture_source = "sessionend:{reason}"` where `reason` comes from
`input.extra.get("reason")`. Keep the existing stderr directive unchanged.
The capture is best-effort (errors are logged to stderr, not fatal).

### New `Cargo.toml` `[[bin]]` entry for `cortex-hooks`:

```toml
[[bin]]
name = "cortex-pre-compact"
path = "src/bin/pre_compact.rs"
```

Add `cortex-episodic.workspace = true` to `[dependencies]`.

### Phase 2 tests (gate)

File: `crates/cortex-hooks/tests/hooks.rs` — extend the existing suite.

```rust
// pre_compact_auto_creates_episode_with_correct_source
// Write a fake transcript file, pipe:
//   {"session_id":"s1","transcript_path":"<path>","trigger":"auto","cwd":"<dir>"}
// to cortex-pre-compact.
// Assert exit 0, no stdout JSON (background hook).
// Assert episodic/manifest.json exists, episode has capture_source="precompact:auto".

// pre_compact_manual_creates_episode_with_manual_source
// Same but trigger="manual", assert capture_source="precompact:manual".

// pre_compact_replay_does_not_double_capture
// Run pre_compact twice with the same transcript size and watermark.
// Assert manifest.episodes still has exactly 1 entry after the second run.

// session_end_capture_backstop
// Pipe session_end with a transcript_path pointing to a real temp file.
// Assert episode file is written with capture_source starting "sessionend:".
```

The hook tests must build binaries first (`cargo build --tests` or
`test::build_binary` pattern already used in `crates/cortex-hooks/tests/hooks.rs`
via the `binary(name)` helper). The existing `run_hook_raw` helper can be reused
without modification for pre_compact since it handles arbitrary binaries.

---

## Phase 3 — Consolidator (Phase A of dreaming)

### Files to create/modify

**New agent:**
```
agents/consolidator.md
```

**Modify:**
```
crates/cortex-hooks/src/bin/session_start.rs   # detect pending episodes, inject directive
crates/cortex-hooks/src/lib.rs                 # add has_pending_episodes() helper
```

### `agents/consolidator.md`

Follows the exact structure of `agents/outcome-recorder.md`:
- YAML frontmatter with `name:`, `description:`, `tools:`.
- Input format section.
- Procedure section (amnesiac-legibility rubric as decision criteria).
- Output format section.
- Rules section.
- When to refuse section.

```
---
name: consolidator
description: Consolidates pending episodes into the long-term ledger by applying the amnesiac-legibility rubric. Promotes only learnings that would be usable by a capable collaborator with zero memory of the session. Triggers on "consolidate episodes", "run consolidation", "process episodes", "session-start consolidation".
tools: Bash, mcp__cortex__tag_learning, mcp__cortex__search_learnings, mcp__cortex__get_learning
---
```

**Amnesiac-legibility rubric** (from spec): promote a learning iff it would be
usable by a capable collaborator with zero memory of the session that produced
it. Drop anything only meaningful in-episode (references to "the file we just
opened", "the approach we tried", "what the user said"). Promote
discoveries/decisions/errors/patterns that are project- or domain-general.

**Consolidation procedure:**

1. Receive the episodic manifest path and list of pending episode IDs from the
   session_start directive.
2. For each pending episode: read the episode JSON; read the transcript slice
   using `byte_range` (if transcript exists and is readable).
3. Apply the rubric to each segment: decide which fragments are promotable.
4. Call `tag_learning` for each survivor (category, content, confidence 0.70).
5. Write `promoted_block_ids` back to the episode record in the manifest
   (set `status = "consolidated_pending_confirmation"`).
6. Output a PROMOTED / SKIPPED summary (same format as outcome-recorder).

**`session_start.rs` extension:**

Add logic after the existing orientation+learnings injection:

```rust
// After build_context(), append consolidation directive if pending episodes exist.
// The source field from hook input determines branch behaviour:
// - source ∈ {"compact", "clear"}: context was lost; consolidation is critical.
//   Directive text: "Context was lost (compaction/clear). Consolidated episode
//   directives follow — process these before other work."
// - source ∈ {"startup", "resume"}: normal start/resume.
//   Directive text: "Consolidation bench: tidy any pending episodes below."
// - source absent / unknown: treat as startup.
//
// Idempotent: if no pending episodes exist in the manifest, append nothing
// (the spec's "zero pending → clean no-op" requirement).
```

In `cortex_hooks::lib.rs` add:

```rust
/// Returns true if the episodic manifest in state_root has any episodes
/// with status Unconsolidated or Consolidating. O(n_episodes) read.
pub fn has_pending_episodes(state_root: &Path) -> bool {
    // load_manifest + check episodes.values().any(|e| e.status == Unconsolidated || Consolidating)
    // Returns false on any I/O error (fail-safe: no-op is better than crashing SessionStart).
}
```

`state_root` is derived the same way as in pre_compact.rs:
`project_dir(input).join("cortex-state")`.

### Phase 3 tests (gate)

Unit tests in `crates/cortex-hooks/src/bin/session_start.rs` (inline `#[cfg(test)]`):

```rust
// consolidation_directive_on_pending_episodes_compact_source
// Seed a manifest with one Unconsolidated episode.
// Call build_context() with source="compact".
// Assert output contains the "context lost" consolidation directive.
// Assert output contains the episode_id.

// consolidation_directive_on_pending_episodes_startup_source
// Same but source="startup".
// Assert output contains the "tidy the bench" directive.

// no_directive_on_zero_pending_episodes
// Manifest exists but all episodes are Evictable.
// Assert output does NOT contain any consolidation directive.

// no_directive_when_no_manifest
// No episodic dir at all.
// Assert output does NOT contain consolidation directive (clean no-op).
```

For the agent (`agents/consolidator.md`): a fixture-based integration test
is out of scope for automated CI (it's a markdown agent). Gate is verified by
hand-running the agent over a labelled fixture episode and measuring promotion
precision per spec success criterion 3 — document the measured precision in a
comment in the agent file.

---

## Phase 4 — Phase B wiring (dream re-index after consolidation)

### Files to modify

```
crates/cortex-hooks/src/bin/session_start.rs   # invoke cortex-dream after consolidation
```

No new files. No changes to `cortex-dream/src/lib.rs` — the existing
`cortex_dream::run(ledger_path, state_path, None)` call is used as-is.

### How

After the consolidator directive is injected in `session_start.rs`, append a
second directive instructing the agent to invoke the `cortex-dream` slash
command after consolidation completes:

```
After the consolidator has finished promoting learnings via tag_learning,
run `/cortex-dream` to regenerate active memory so the new learnings are
reflected immediately.
```

This is a textual directive in `additionalContext` — no binary code calls
`cortex-dream` directly. The agent follows the instruction.

Note: this preserves the no-API, no-daemon constraint. `cortex-dream` is a
registered slash command (in `commands/cortex-dream.md`) which the live agent
executes via the existing mechanism.

### Phase 4 tests (gate)

```rust
// session_start_includes_dream_directive_after_consolidation
// Seed manifest with pending episodes, source="compact".
// Assert output contains both the consolidation directive AND a reference
// to "cortex-dream" (the re-index instruction).

// existing_dream_tests_still_pass
// Run cargo test -p cortex-dream — all existing tests in
// crates/cortex-dream/tests/pipeline.rs must still pass unchanged.
// (This is a regression gate, not a new test.)
```

---

## Phase 5 — Lazy outcome-gated eviction

### Files to create/modify

**Modify:**
```
crates/cortex-hooks/src/bin/session_start.rs   # call reconcile+prune after consolidation
crates/cortex-episodic/src/eviction.rs         # already created in Phase 1 — integrate with Phase 3 state
```

### How

In `session_start.rs`, after the dream re-index directive is appended, add a
reconcile-and-prune step:

```rust
// Load the manifest (already loaded for pending-episode check).
// Load ledger reinforcements.
// Call cortex_episodic::reconcile_eviction(manifest, &reinforcements, TTL_DAYS=30).
// Call cortex_episodic::prune_evictable(state_root, &mut manifest).
// Save updated manifest.
// This runs synchronously in the hook binary (fast: no transcript reads).
```

`TTL_DAYS = 30` as a named constant; tunable later from observed episode sizes.

The reconcile-and-prune runs on every `session_start` regardless of source.
It is idempotent: episodes already Evictable are already pruned; a repeated
call is a no-op.

### Phase 5 tests (gate)

File: `crates/cortex-episodic/tests/phase5.rs` (or extend `phase1.rs`).

```rust
// episode_not_pruned_before_blocks_confirmed
// Seed manifest with one ConsolidatedPendingConfirmation episode,
// promoted_block_ids=["block-X"]. Reinforcements has block-X with no outcomes.
// TTL not yet exceeded. Call reconcile_eviction. Assert episode stays
// ConsolidatedPendingConfirmation, prune_evictable removes 0 files.

// episode_pruned_after_success_outcome
// Same setup but block-X has a Success outcome. Call reconcile_eviction.
// Assert episode becomes Evictable. Call prune_evictable.
// Assert episode file is removed from disk and episode is removed from manifest.

// episode_pruned_after_ttl_regardless_of_outcomes
// Episode with no outcomes, captured_at 31 days ago, TTL=30.
// Assert becomes Evictable and is pruned.

// episode_retrievable_before_eviction
// Assert that episode files exist on disk and can be read back as EpisodeRecord
// before prune_evictable runs (fulfils "pre-confirmation episodes remain on disk
// and retrievable").
```

---

## Verification: spec success criteria → concrete checks

| # | Spec criterion | Concrete check |
|---|---|---|
| 1 | PreCompact captures unconsolidated tail with correct byte range and capture_source | Phase 2 test `pre_compact_auto_creates_episode_with_correct_source` |
| 2 | Resume / replay does not double-capture | Phase 2 test `pre_compact_replay_does_not_double_capture` |
| 3 | Consolidator promotes only amnesiac-legible learnings | Phase 3 hand-run over fixture: measure precision, document in `agents/consolidator.md`. Threshold: do not gate on a specific number — report measured precision honestly (spec criterion 7). |
| 4 | After SessionStart, new learnings in ledger + in active-memory snapshot | Phase 4 test `session_start_includes_dream_directive_after_consolidation` + manual run verifying dream snapshot contains promoted IDs. |
| 5 | Episode pruned only after blocks confirmed or TTL elapsed | Phase 5 tests `episode_not_pruned_before_blocks_confirmed`, `episode_pruned_after_success_outcome`, `episode_pruned_after_ttl_regardless_of_outcomes`. |
| 6 | Zero pending → clean no-op | Phase 3 test `no_directive_on_zero_pending_episodes` |
| 7 | Honest failure on low consolidator precision | The agent's rules section must explicitly state: "If promotion precision on a fixture is below expectation, report the measured rate — do not force promotions to inflate metrics." |

---

## Codebase discrepancies / spec–reality gaps

1. **`write_atomic_json` visibility**: the spec says to factor shared
   dir+pointer/atomic-write helpers from `cortex-handoff` into `cortex-core`.
   In practice, `cortex-core/src/objects.rs` already has `write_atomic_json`
   as `pub(crate)`. The shared-helpers refactor is a visibility promotion +
   extraction of the pointer pattern, not a ground-up write.

2. **`source` field in `SessionStart` hook input**: `session_start.rs` currently
   reads `session_id`, `cwd`, and `transcript_path` from `HookInput`, but does
   not read a `source` field. The spec requires branching on
   `source: startup|resume|clear|compact`. This field will need to be added to
   `HookInput.extra` parsing in `cortex_hooks::lib.rs` (or added as an explicit
   field on `HookInput`). Verify the exact field name and values against Claude
   Code hook docs at implementation time.

3. **`background: true` in hooks.json**: the current `hooks/hooks.json` has no
   `"background"` key on any hook entry. Verify the Claude Code hook schema for
   `PreCompact` async declaration before committing the hooks.json change.

4. **`cortex-dream` invocation path**: the spec says Phase B is invoked "after
   Phase A promotes within a SessionStart pass." In the actual codebase,
   `cortex-dream` is a standalone binary (slash command), not a library function
   callable from the hook binary. The hook can only emit text directives;
   it cannot exec `cortex-dream` directly from within `session_start.rs`. The
   implementation must therefore emit a text directive to the agent ("run
   `/cortex-dream`") — this matches the no-API constraint and the existing
   markdown-agent dispatch pattern.

5. **`transcript_path` field presence**: `HookInput.transcript_path` exists
   (`cortex_hooks::lib.rs` line 25) but Phase 2 hooks must handle the case
   where it is None. The spec notes transcript fidelity is an open question;
   `capture_tail` must treat None as a graceful no-transcript-path case.

6. **`cortex-state` root path**: `cortex-dream/src/main.rs` uses
   `ledger_path.join("cortex-state")` as the state root. Hook binaries derive
   state root from `project_dir(input)` → the state root convention used by
   `cortex-episodic` must match what `cortex-dream` uses. Use
   `project_ledger_path(project_dir).join("cortex-state")` (i.e.,
   `<cwd>/.claude/cortex/ledger/cortex-state`) to be consistent with the
   dream binary's default.

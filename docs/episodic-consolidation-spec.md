# cortex — Episodic Memory & Consolidation (the real "dream")

Spec of record for the episodic tier and the STM→LTM consolidation pathway. Additive on the v4 workspace. Follows the conventions of `docs/v4-plan-of-record.md`: decisions baked in with rationale, ordered phases with gates, what stays inviolable, honest open questions.

## Why this doc exists

`cortex-dream` today reads the ledger, builds the BM25 graph, eigendecomposes, and writes an active-memory snapshot. That is **LTM → LTM re-indexing**. There is no short-term store, so there is nothing it consolidates *from* — and the only way a learning enters the ledger is an explicit `tag_learning` judgment call mid-session. That makes the ledger's growth a measure of *tagging activity*, not *experience*, and it means the bulk of a session's episode evaporates when the context window clears.

This spec adds the missing tier and redefines dreaming as what the metaphor always implied: the consolidation transfer from a raw episodic buffer into the durable ledger.

**Organizing principle (the brilliant friend at the workshop).** Continuity does not live in the worker. Each Claude instance is the capable collaborator who does not remember you, the path, or the in-between — and still produces great work, because the *workshop* holds the durable artifacts and the resumption state. So: the in-between is allowed to be forgotten; the work is not. The single governing rule for what survives is the **amnesiac-legibility test** — *promote a learning iff it would be usable by a capable collaborator with zero memory of the session that produced it.* Anything meaningful only in-episode stays in the episodic buffer and is evicted.

## The memory model (three tiers)

| Tier | Crate | Role | Brain analog | Lifecycle |
|---|---|---|---|---|
| Episodic (NEW) | `cortex-episodic` | Raw session episode, captured automatically. The *input* to consolidation. | Hippocampal trace | Fast, capacity-bounded, **evicted after consolidation is confirmed** |
| Durable | `cortex-core` ledger | Consolidated, signed, decaying patterns. The durable *output*. | Neocortex | Slow, permanent, 180-day confidence half-life |
| Working set | `cortex-active-memory` + `cortex-handoff` | Read-side products: salient projection of the ledger, plus the resumption note. | Working memory + episodic-buffer-as-note | Regenerated per dream / per save |

`cortex-active-memory` already occupies the working-memory role. The new store is therefore **not** "short-term memory" in the working-memory sense — it is the *episodic-consolidation buffer*. Hence `cortex-episodic`, not `cortex-stm`; the latter name would falsely imply it competes with active-memory.

"Dream" is now two phases:
- **Phase A — consolidate (NEW):** read pending episodes, apply the amnesiac-legibility rubric, promote survivors into the ledger. Requires judgment → a Claude pass.
- **Phase B — re-index (existing):** the current spectral pipeline, run over the updated ledger → active-memory + spectrum snapshot.

## What stays inviolable

- **Ledger block format** — byte-for-byte unchanged. All episodic↔block back-references live in the episodic manifest, never as new fields on blocks.
- **MCP tool signatures** — promotion uses the existing `tag_learning`; confirmation reads existing `record_outcome` state.
- **No-API constraint** — consolidation never calls the Anthropic API or any external model. Phase A runs **inside the live Claude Code session** via the existing markdown-agent dispatch mechanism (same class as `agents/outcome-recorder.md`). No daemon, no headless orchestration, nothing beyond the user's existing Claude Code subscription.

## Event surface (verified against Claude Code hook docs, June 2026)

| Event | Fires | Key field | We use it for |
|---|---|---|---|
| `PreCompact` | Just before compaction (`/compact` or auto when window fills) | `trigger: manual\|auto` | **Capture (primary).** Auto-compaction is the platform's native "episodic buffer full" signal. Transcript is still full-fidelity here; after compaction the in-between is summarized away. |
| `SessionEnd` | Clean close | `reason: clear\|logout\|prompt_input_exit\|other` | **Capture (clean close).** Backstop for sessions that end without compacting. |
| `SessionStart` | Startup, **resume, clear, compact** | `source: startup\|resume\|clear\|compact` | **Consolidate (dispatch).** Re-fires on resume and after clear/compact, so it covers every re-entry case. Branch behaviour on `source`. |

**Idempotency hazard (must handle):** on `--continue`/`--resume`, Claude Code *replays* saved hook output rather than re-running mid-session hooks. Capture and consolidation must therefore be watermark-gated so a replayed `SessionStart` directive no-ops when nothing is pending and capture never re-copies already-captured bytes.

**The benign non-event:** a live session that never closes, compacts, or clears fires nothing — but has lost nothing either (the whole episode is still in context). Consolidation simply waits for the next boundary. This is correct, not a gap. Pressure matters only when something is about to be discarded.

## Data shapes

`<state-root>/episodic/` mirrors the existing `handoffs/` + `spectrum-history/` dir+pointer convention.

```
episodic/
├── episode-{session}-{rfc3339-z}.json   # captured episode tail (raw)
└── manifest.json                        # consolidation state + watermarks
```

Episode record:
```jsonc
{
  "episode_id": "uuid",
  "session_id": "abc123",
  "captured_at": "2026-06-02T...Z",
  "capture_source": "precompact:auto",   // precompact:{auto|manual} | sessionend:{reason}
  "transcript_path": "/path/to/transcript",
  "byte_range": [start_offset, end_offset],  // slice of transcript captured
  "status": "unconsolidated",            // unconsolidated | consolidating
                                         // | consolidated_pending_confirmation | evictable
  "promoted_block_ids": [],              // populated by Phase A
  "custom_instructions": null            // PreCompact may carry /compact "preserve X"
}
```

Per-session watermark (in `manifest.json`): `consolidated_through_offset` — the transcript byte offset already captured/consolidated, so capture only ever appends the new tail and replay never double-counts.

## Phases (ordered, each gated)

| # | Scope | Gate |
|---|---|---|
| 1 | **`cortex-episodic` crate**: manifest + episode types; `capture_tail(transcript_path, watermark) -> EpisodeRecord`; `reconcile_eviction(ledger)`. Factor any dir+pointer/atomic-write helpers duplicated with `cortex-handoff` into `cortex-core`. | Round-trip capture writes correct `byte_range`; a second capture at the same watermark is a no-op. Eviction reconcile is a pure function of manifest + ledger state. |
| 2 | **Capture hooks**: new `cortex-pre-compact` bin; extend `session_end.rs` to capture; register `PreCompact` in `hooks/hooks.json` (async, non-blocking). | PreCompact and SessionEnd produce episode files with correct ranges; re-fire / resume-replay does **not** double-capture (watermark holds). Hooks stay well under their cold-start budget. |
| 3 | **Consolidator (Phase A)**: `agents/consolidator.md` carrying the amnesiac-legibility rubric; `session_start.rs` detects pending episodes and injects a consolidation directive **keyed on `source`** (compact/clear → "context lost, here is what was consolidated"; startup/resume → "tidy the bench"). Consolidator promotes survivors via `tag_learning`, writes `promoted_block_ids` back to the manifest, marks episodes `consolidated_pending_confirmation`. | Promotes only learnings that pass the amnesiac test; drops in-episode-only material. Idempotent on replay (no pending → no-op). No API/model call outside the live session. |
| 4 | **Phase B wiring**: after Phase A promotes within a SessionStart pass, run the existing `cortex-dream` re-index so active-memory reflects new learnings. | Newly promoted learnings appear in the regenerated active-memory snapshot and the spectrum snapshot. All existing `cortex-dream` tests still pass. |
| 5 | **Lazy outcome-gated eviction**: at consolidation time, reconcile `consolidated_pending_confirmation` episodes — an episode becomes `evictable` once each of its `promoted_block_ids` has earned a positive `record_outcome` **or** aged past a TTL backstop without contradiction; then prune the raw file. | An episode is never pruned before its promoted blocks are confirmed-or-TTL'd. No live hook into `record_outcome` (lazy reconcile only; ledger format untouched). |

## Constraints

- Rust 1.85+; add `cortex-episodic` to the workspace `Cargo.toml` members; library deps via `cargo add`.
- **Backward compatibility:** all existing workspace tests must pass; ledger on-disk format unchanged; `cortex-dream` behaviour unchanged except for being invoked after Phase A.
- **No-API:** no network calls, no Anthropic SDK, no headless `claude -p`. Consolidation is agent-dispatched inside the live session only.
- PreCompact hook must be async / must not block compaction.
- Each phase ships its own tests (do not defer testing to the end).

## Success criteria

1. A PreCompact event captures the unconsolidated transcript tail into `episodic/` with a correct byte range; auto vs manual recorded in `capture_source`.
2. Resuming a session does not re-capture or re-consolidate already-processed bytes (watermark verified).
3. The consolidator, run over a fixture episode, promotes only amnesiac-legible learnings and leaves in-episode-only noise unpromoted — verified against a hand-labelled fixture.
4. After a SessionStart consolidation pass, the new learnings are in the ledger (via `tag_learning`) and reflected in a freshly regenerated active-memory snapshot.
5. An episode is pruned only after its promoted blocks are confirmed via `record_outcome` or pass the TTL backstop; pre-confirmation episodes remain on disk and retrievable.
6. With zero pending episodes, SessionStart consolidation is a clean no-op.
7. **Honest-failure:** if consolidator promotion precision on the fixture is poor, report the measured precision rather than forcing promotion — degraded precision is a finding, not a failure to hide.

## Open questions (revisit with real data)

- **Transcript fidelity:** does the Claude Code transcript persist reasoning/thinking, or only messages + tool I/O? Determines how rich the salience signal is. Verify during Phase 2; the design is robust either way (we stage whatever is present).
- **Consolidator precision:** the `post_tool_use` note records a 43% snippet-distillation failure rate for naive extraction. The safety net is outcome + 180-day decay: over-promoted junk is never reinforced and decays out; correct promotions get exercised and converge. Measure real precision once running and decide whether the rubric needs tightening.
- **Batching / chunking:** when pending episode volume exceeds one consolidation-pass context budget (the natural setpoint for "how much before we must consolidate"), process oldest-first across multiple passes. Tune the budget from observed episode sizes.
- **Clear semantics:** `/clear` is a *deliberate* discard. Should it consolidate-then-evict more aggressively than the lazy default, since the user has signalled the in-between is done?

## Notes (strategic context)

This is the primary ingestion path going forward — manual `tag_learning` becomes the exception (a human-flagged promotion), not the rule. The episodic tier is deliberately its own crate because it is *foundational and will grow*: future intelligence — better salience detection, episode replay, cross-episode pattern mining, and eventually self-model work — all operate on the raw episode stream. `cortex-handoff` is bounded and effectively done; it must not absorb a growing foundation. Shared persistence mechanics, if duplicated, belong in `cortex-core` (which exists precisely to hold what grows shared).

The whole design rests on one line: **the forget is safe as long as the work provably remains before the worker dissolves.** PreCompact is where the worker dissolves; capture-before-loss plus confirmation-before-eviction is what makes the forgetting honest. If consolidation quality cannot be made trustworthy, the correct fallback is *not* to force promotion — it is to keep the episodic buffer as a retrieval fallback (search recent episodes directly) and leave promotion to manual `tag_learning`, until precision earns the automation. Cheap honest version first; earn the rest.

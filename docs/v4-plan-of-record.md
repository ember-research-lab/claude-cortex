# cortex v4 — Plan of Record

Drafted 2026-05-07 during the v0.3.1 patch cycle. Source spec: `cortex-v4-spectral.md`.

## Why this doc exists

The spec defers six design decisions to "real v3 usage data." For Aaron's situation (mostly solo + a few internal users), waiting weeks for that data isn't going to produce a meaningful sample. **Self-introspection during active sessions is the more useful signal source.** v4 leans into that explicitly: build cortex-monitor early, treat its observations as a primary input, and re-tune the deferred decisions from cortex's own spectral history rather than abstract usage stats.

This doc records the defaults baked in, the order of work, and the gates that block progression.

## Defaults baked in

| Decision | Spec default | v4 default | Reason |
|---|---|---|---|
| Embedding source | API vs local (deferred) | **Anthropic API** | Better quality; cost amortized by content-addressed cache; latency acceptable since hooks already accept Anthropic-network round-trips. |
| Edge weight composition | 0.4 / 0.4 / 0.2 | **0.4 / 0.4 / 0.2** | Spec default. Tunable from `cortex-state/active/active-*.json` records once enough dreaming passes have run. |
| Top-k eigenmodes | 50 | **`min(50, n / 3)`** | Floors at 1, scales for small ledgers. Spec default at scale, scaled-down for small ledgers so we don't try to extract 50 modes from a 30-node graph. |
| Dreaming trigger | Manual only | **Manual only initially** | Slash command `/cortex-dream`. Cron / session-end hook deferred until dreaming proves it adds value. |
| Update strategy | Full recompute | **Full recompute** | Lanczos for n ≥ 500, full eigendecomposition below. Incremental updates deferred until full recompute actually exceeds the 60s budget at observed scale. |
| Self-monitor thresholds | Calibrated from history | **Calibrated, not hardcoded** | Phase 7 (cortex-monitor) lands earlier in v4 than the spec orders, precisely so we have history to calibrate from before tightening anything. |

## Ordered phases

Reordered from the spec to land introspection earlier:

| # | Crate / phase | Lands before | Gate |
|---|---|---|---|
| 1 | `cortex-embeddings` | spectral can compute similarity | Anthropic API call works against the lab account; cache hits don't re-embed. |
| 2 | `cortex-spectral` (graph + Laplacian + eigendecomp) | active-memory | Eigendecomposition completes for a 100-node mock graph in <1s on a laptop. |
| 3 | `cortex-monitor` (moved earlier from spec Phase 7) | active-memory + dream | Records spectrum snapshots given a static graph. Detection logic stubbed — implementation gates on having ≥3 dreaming-pass snapshots. |
| 4 | `cortex-active-memory` | dream | `build_active_memory(graph, eigendecomp, k)` produces a deterministic snapshot; `current` symlink updates atomically. |
| 5 | `cortex-dream` (binary + slash command) | spectral retrieval | Dreaming pass under 60s for an n=500 mock ledger; emits monitor snapshot. |
| 6 | Spectral confidence (cortex-mcp updates) | spectral retrieval | `Confidence(entry) = Σ_i λ_i · proj_i(entry)²` matches scalar v3 confidence on a controlled dataset where they should agree (no spectral structure → fallback to scalar). |
| 7 | Spectral retrieval (cortex-mcp updates) | release | **GATE**: resonance retrieval ≥ tag retrieval on representative test queries. If it doesn't outperform, the spectral approach has a real problem to investigate, not paper over. |
| 8 | Validation + release | — | **GATE**: `top-k by spectral resonance` is materially different from `top-k by raw embedding cosine`. If they're functionally identical, eigenstructure isn't earning its complexity — investigate edge weight composition. |

## Crate layout (additive on v3)

```
crates/
├── cortex-core/              # v3 unchanged
├── cortex-mcp/               # v3, minor updates Phase 6/7
├── cortex-hooks/             # v3, minor updates to read from active memory
├── cortex-migrate/           # v3 unchanged
├── cortex-embeddings/        # v4 NEW — Phase 1
├── cortex-spectral/          # v4 NEW — Phase 2
├── cortex-monitor/           # v4 NEW — Phase 3 (moved earlier)
├── cortex-active-memory/     # v4 NEW — Phase 4
└── cortex-dream/             # v4 NEW — Phase 5
```

## What stays inviolable from v3

- Ledger format (block JSON, index, reinforcements, merkle, identity) byte-for-byte.
- MCP tool *signatures* (internals can switch from scalar to spectral; the schema stays).
- Confidence semantics at write time: outcomes still call `record_outcome` with success/partial/failure, deltas still apply at the scalar level. The spectral layer derives its own confidence at *read* time and ignores the scalar value when a spectrum exists.

## Open questions to revisit after Phase 3 (cortex-monitor)

- Is the spectrum stable across consecutive dreaming passes? If yes, we can tighten top-k. If no, investigate whether outcome correlation is too noisy.
- What's the typical spectral gap (λ_2 − λ_1) for Aaron's actual ledger? Tells us how much room the dominant subspace has to grow.
- Are eigenvector signs stable across passes? If they flip, retrieval is non-deterministic and we need to canonicalize signs.

## Out of scope (v5+)

- Online spectral updates (recompute on every ledger append) — too expensive for v4 budget.
- Cross-user / federated learning graphs — privacy + security implications deserve their own design pass.
- Topological invariants beyond spectrum (persistent homology, sheaf cohomology) — possibly powerful, certainly premature for v4.
- Cortex self-analysis as input to AI alignment research — research direction, not a v4 feature, but the framework should not preclude it.

## Implementation status (as of 2026-05-07)

- v4 branch exists at `ember-research-lab/claude-cortex` (no commits yet beyond branchpoint at v0.3.0 + cleanup).
- Embeddings + spectral crate scaffolds drafted and reverted from main during v0.3.1 cleanup. Need to be re-applied to the v4 branch fresh.
- Monitor / active-memory / dream: not yet drafted.
- This doc is the first artifact to land in v4 work.

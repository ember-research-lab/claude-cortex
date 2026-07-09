# cortex v-next — Unified Memory + Project Substrate (spec)

**Status:** draft / design-of-record. Extends — does not replace — `v4-plan-of-record.md`
(spectral retrieval) and `episodic-consolidation-spec.md` (transcript→ledger). Reconciled
against dev repo **v0.4.0** (`main`, clean tree). Confidence/parameter values are eyeball
defaults **pending calibration** (see §9).

## 0. North star

cortex exists to make the agent work **easier, more token-efficient, and smarter**. Every
decision below is judged against three questions: does it use *fewer tokens*, surface the
*righter context*, and improve *reasoning*? `ember-graph` was purpose-built for this — its
token-budgeted `query(question, depth, budget_chars) → compact subgraph` is that intent in
code. This spec finishes wiring it in.

## 1. Three substrates

| Substrate | Holds | Type | Source of truth for |
|---|---|---|---|
| **Transcript** (`~/.claude/projects/*.jsonl`) | everything that happened, verbatim | episodic | "what happened" |
| **Ledger** | durable insights / structural patterns, esp. reasoning-born | semantic | "what was understood" |
| **ember-graph** | typed entities + relations (memory **and** code), provenance, confidence | relational | "how things connect" + smart retrieval |

Key principle (from `episodic-consolidation-spec.md:110`, affirmed): **the ledger is insight
memory, NOT a transcript projection.** The highest-value entries are thin-or-absent in the
transcript — insights formed during reasoning. Transcript mining *recovers* durable patterns
that flew by untagged; it is not the whole capture path.

## 2. The unified graph

**One graph, two interlinked node families**, on the shared `ember-graph` substrate:

- **Memory nodes** — `learning`, `outcome`, `episode`, `entity`.
- **Code nodes** — `file`, `module`, `function`, `type`, `service`.
- **Join edges** — `learning_about` (insight → code node), `corroborates`, `supersedes`,
  `defined_in`, `calls`, `imports`, `depends_on`, `data_flow`.

Query *"what do I know about module X"* traverses **both** families in one hop — code
structure + every insight/outcome attached to it. `ember-graph`'s schemaless typed nodes
(`kind: String`, `source: Option<String>`) and content-addressed `NodeId::canonical(ns,
entity)` already support this; namespacing keeps collision domains clean
(`code::whale-signal::store.rs` vs `learning::ddad7500`).

## 3. Confidence model

Replaces the flat scalar + 180-day decay-to-zero (`cortex-core/src/confidence.rs:84-85`).
Three separated roles:

### 3.1 Origin enum (epistemic, slow-moving)
How the fact was *born*; sets prior + dynamics.

| origin | prior p0 | half-life h | α⁺ | α⁻ | notes |
|---|---|---|---|---|---|
| Extracted | 0.90 | 365d | 0.02 | — | observation; failure → **Contested**, not decrement |
| Inferred(near) | 0.65 | 120d | 0.10 | 0.15 | hypothesis; symmetric, movable |
| Inferred(far) | 0.50 | 60d | 0.10 | 0.15 | rots faster |
| Ambiguous | 0.35 | 30d | high | high | resolve-or-forget (hard TTL) |
| Validated | 0.90 | 365d | 0.02 | 0.15 | earned observation-grade durability |
| Contested | 0.20 | — | — | — | quarantined from default retrieval; kept for audit |

### 3.2 Live empirical confidence (usage-driven, fast-moving)
Multiplicative updates (auto-bounded, diminishing at the rails); α is enum-conditioned:

```
Success:        c ← c + α⁺ · (1 − c)
Failure:        c ← c − α⁻ · c
Corroboration:  c ← c + β  · (1 − c) ;  corroboration += 1        (β ≈ 0.05)
any update:     last_reinforced ← now
```

Corroboration counts toward confidence **directly** (not only via applied outcomes) — it is
the passive signal that accrues when explicit `record_outcome` is neglected, and the only
realistic path to a calibration corpus.

### 3.3 Decay relaxes toward the prior (the linchpin)
```
effective_c(t) = p0(origin) + (c − p0) · 2^(−Δt / h(origin)) ,  Δt = now − last_reinforced
```
Disuse erodes the trust **usage earned**, not the intrinsic epistemic warrant. Half-life
measures *time since last useful application*, not creation age.

### 3.4 Reclassification (usage moves the enum)
```
Inferred ──[ effective_c ≥ 0.85 AND corroboration ≥ 3 ]──▶ Validated
Extracted/Validated ──[ Failure OR effective_c < 0.25 ]──▶ Contested   (close valid_until, open successor)
Ambiguous ──[ crosses threshold ]──▶ Extracted/Inferred ;  ──[ low past TTL ]──▶ forgotten
Contested ──[ re-confirmed ]──▶ Validated/Extracted
```
A confidence collapse from repeated failure **is** the bi-temporal supersede trigger (§4).

### 3.5 Trust on read (for ranking)
`trust = effective_c · origin_weight(origin)`, `Contested` excluded by default, **graph
centrality as a secondary booster** — fills dream's stubbed `outcome_correlation` and is the
concrete form of the v4 "confidence from spectral structure" intent. Culling of never-used
clutter happens via low centrality, not a timer.

### 3.6 Migration
Old v3/v0.4 blocks lack `origin` → default `Inferred(near)` on read (`Extracted` if `source`
is the user); `confidence` carries over; `corroboration = 0`. Reads old ledgers without
rewrite.

## 4. Bi-temporal + provenance

- **Edges carry** `created_at`, `valid_from`, `valid_until: Option`. A new assertion about the
  same (source, target, relation) **appends and supersedes** (closes prior `valid_until`)
  rather than mutating in place — history preserved. `query`/`neighbors`/`fact_view` gain an
  as-of filter.
- **Code facts are naturally commit-bounded**: *"foo calls bar, valid commit X→Y"* — the same
  machinery serves "retract overclaims" (memory) and "track structure across commits" (code).
- **Provenance**: `ember-graph`'s signed envelope (already reuses cortex Ed25519/BLAKE3,
  `ember-graph/src/provenance.rs:16-19`) records `asserting_agent` + `basis` + source
  content-hash. Extend with a structured locator (transcript span: `doc_id`, `(start,end)`)
  and a float confidence. **`cortex-audit`** (signed, hash-chained action ledger) is the
  natural home to record outcomes/corroborations as *audited actions* — tamper-evident
  capture for free. `asserting_agent` makes "insight born during reasoning session S" a
  first-class provenance source (not only document extraction).

## 5. Auto-capture — the fuel line

A usage-driven model is worthless without automatic capture (see ledger `ddad7500`). Today
`cortex-hooks classify()` (`post_tool_use.rs:161-169`) nudges only `tag_learning` on
`WebFetch`/`WebSearch`/external-mcp — never `record_outcome`, never `Task`/`Agent`.

- **`record_outcome` nudge** when a retrieved learning (`search_learnings`/`get_learning`) is
  followed by real work.
- **Corroboration nudge/event** on re-observation of a known fact.
- **`Task`/`Agent` completion** added to the substantive-tool allowlist — subagent syntheses
  are the richest durable content and currently get nothing.
- **Distillation is agent-driven and auto-fired.** Deciding what's durable is the
  pattern-vs-state *judgment* — a reasoning task, kept in the `consolidator.md` agent (not a
  deterministic code miner). The gap is reliable triggering: fire the consolidator on
  session-end / pre-compact rather than depending on recall. It MAY offload mining to Grok
  (see ember-grok).

## 6. Global + local tiers

Both graphs are two-tiered, mirroring the existing ledger split (`resolve_ledger`,
`impls.rs:36-47`):

- **Local** (`<project>/.claude/cortex/…`) — this repo's code graph + project-specific
  insights. Loaded when working there.
- **Global** (`~/.claude/cortex/…`) — cross-project knowledge + a lightweight **portfolio
  map** (cross-repo edges no local graph can hold: whale-signal-core → mobile/desktop/model
  via UniFFI; shared crates; ember-graph↔cortex).

- **Merged scoped query view**: in project P, query `P-local ∪ relevant-global`; never pull
  sibling projects' locals. This *is* the relevance/token mechanism — scoping makes context
  relevant by construction (fixes confidence-ranked-but-irrelevant surfacing). Uses
  `ember-graph::merge` + `project_dir`.
- **Promotion**: a local insight **corroborated across N distinct projects** auto-promotes
  local→global — the generalization signal is just `corroboration` with a project-diversity
  condition. (Currently the deferred "cross-project recommender" stub, `impls.rs:643`.)

## 7. Project / code graph

- **Extraction reuses an existing parser** (tree-sitter for polyglot breadth / LSP / the
  `modernize-map` call-graph tooling / Sourcegraph) — not hand-rolled. `ember-graph` states
  extraction is the consumer's job.
- **Incremental maintenance** via a `PostToolUse` hook on `Edit`/`Write` (update affected code
  nodes) + a git-commit hook (close/open temporal edges) — the same "capture on use, don't
  rebuild" philosophy as memory capture.
- **Confidence applies**: parsed `calls` edge = `Extracted` (commit-superseded); inferred
  "these modules form a subsystem" = `Inferred`.
- **Dogfood order**: cortex + ember-graph themselves first (small, in-hand, we map the tool
  with the tool); whale-signal second (biggest token-pain, real payoff).

## 8. Roadmap (phases + gates)

| Phase | What | Layer | Gate (works-as-expected) |
|---|---|---|---|
| 0 · Setup + spec | this doc; build loop confirmed (`install.sh`) | docs | builds clean; both repos edit→rebuild verified |
| 1 · Capture fuel line | `classify()`: `record_outcome` + corroboration + `Task`/`Agent`; auto-fire consolidator | mutable | outcomes record automatically; Agent syntheses nudged; reinforced patterns log events |
| 2 · Smart retrieval | project ledger→ember-graph; serve surfacing via token-budgeted relevance query | mostly mutable | surfaced context *relevant* + **measurably smaller** (token delta reported) |
| 2b · Project graph | extract cortex+ember-graph → code nodes; incremental hooks | mutable+ | answers "what depends on X / where is Y / what do I know about Z" from graph, cheaper than grep, stays fresh |
| 3 · Confidence model | origin enum + usage-driven + corroboration + trust-on-read; reads old ledgers | **substrate (major)** | old reads unchanged; reinforced patterns **climb not decay**; 0.7 fact ≈ 70% success |
| 4 · Temporal + provenance | bi-temporal edges + supersede; provenance via cortex-audit; lift cortex-core locked store into ember-graph | substrate | contradicted fact **supersedes** (history kept); provenance traces to source |
| 5 · Grok + synthesis e2e | ember-grok auto-bridge feeds capture; verifier verdicts drive confidence | integration | Grok verifies → outcome recorded → confidence moves → surfaced smarter next session |

Sequencing: mutable-layer wins (1, 2, 2b) first — real value, low risk, and they accumulate
the outcome/corroboration corpus that Phase 3 calibration needs. Substrate changes (3, 4) only
after. Each phase ships standalone with a falsifier gate; run producer → verifier per phase.

## 9. Reconciliation with v0.4.0 (what exists vs net-new)

- **Confidence (§3):** net-new. `cortex-confidence` is the SMB `ConfidenceOracle`
  generalization, **not** this epistemic model; core math still decays to zero with fixed
  deltas.
- **Episodic (§1, §5):** half-built & aligned. Capture (byte-range watermarks), episode /
  manifest / eviction, and `consolidator.md` exist; **auto-triggering** of distillation is the
  gap. Design matches `episodic-consolidation-spec.md`.
- **Auto-capture (§5):** net-new (unchanged vs 0.3.0).
- **Unified graph + bi-temporal (§2, §4, §7):** net-new. ember-graph is not yet a workspace
  dependency (only ember-crypto); only a memory-only spectral graph exists; entity/cross-project
  graph is a deferred stub.
- **Global/local (§6):** tiers exist (either/or); merged view + promotion net-new.

## 10. Open items / calibration

- Parameters in §3.1 are **eyeball defaults pending calibration** from real ledger outcome
  history (`p0` = per-origin base success rate; `h` = fit to when facts start failing; `α` =
  fit so a confidence-0.7 fact succeeds ~70%). Calibration is infeasible until Phase 1 lands
  and the corpus grows — use defaults until then.
- Confidence-model reconciliation of `ember-graph`'s bucketed enum vs cortex's numeric decay:
  resolved here (enum = origin; numeric = live empirical) — verify no consumer depends on the
  old bucket semantics.
- Whether `cortex-audit` becomes the outcome/corroboration log (recommended) or capture stays
  in the learning ledger.

# Spec-as-work-order — the durable handoff artifact

**Status:** design / decision (2026-06-11). Resolves "work item #3" of the shared-loop build
(`ember-smb-platform/design/shared-loop-architecture.md`): *elevate the spec/handoff to a first-class
durable artifact* so a cheaper implementer can execute against an orchestrator's plan without
re-deriving context. It also **corrects** that doc's wording in light of cortex's pattern-vs-state
discipline.

## The correction: a spec is *state*, not a durable ledger pattern

The shared-loop doc said "elevate the spec to a durable **ledgered** artifact." Against cortex's own
substrate discipline that's half-wrong, and the distinction matters:

- A **task-specific spec** (this plan, for this feature) is **state** — it goes stale once the task is
  done. State belongs in the **handoff** tier (append-only, retrievable), **never** the immutable
  learning ledger. Tagging it as a durable learning would pollute the substrate exactly the way the
  pattern-vs-state audit (v0.3.6) warned against.
- The **spec→outcome *pattern*** ("specs of shape X, by orchestrator Y, succeed") **is** durable — and
  it reaches the ledger the right way: via **episodic consolidation**, not by tagging the raw spec.

So "durable artifact" for a spec means **durably *retrievable*** (an append-only, content-addressed
work-order an implementer can fetch), **not** "a confidence-decaying ledger learning."

## How it maps onto cortex's three existing tiers (no new substrate)

| Tier | Crate | Role for the spec |
|---|---|---|
| **Handoff** (state) | `cortex-handoff` | **The spec lives here** as a structured *work-order* — the orchestrator's intent + acceptance criteria the implementer executes against, and the verifier checks. |
| **Episodic** (bridge) | `cortex-episodic` | Captures the spec's execution as an `EpisodeRecord` (transcript slice), outcome-gated; consolidation promotes the *pattern*, not the spec. |
| **Ledger** (durable pattern) | `cortex-core` | Receives only the consolidated **spec→outcome learning** (durable, confidence-decaying). |

The gap is in the **handoff** tier: today `Handoff` carries task lists + a **free-form**
`context_notes` string. That's a pause-note, not a *work-order* an implementer can execute against. The
spec-as-artifact insight is to make the orchestrator's plan **structured and first-class** there.

## The change (proposed, backward-compatible)

Add an **optional structured spec** to `Handoff` — a work-order:

```text
WorkOrder {
    goal: String,                 // what to achieve, in one line
    intent: String,               // the orchestrator's plan / approach (the "geometry")
    acceptance: Vec<String>,      // falsifier/acceptance criteria — how the verifier confirms it
    scope_files: Vec<String>,     // the files/areas in play (keeps a cheap implementer on-rails)
    non_goals: Vec<String>,       // explicit out-of-scope (prevents over-reach)
}
Handoff { …existing…, #[serde(default)] work_order: Option<WorkOrder> }
```

- **Backward-compatible:** `#[serde(default)]` → existing handoffs (no `work_order`) deserialize
  unchanged. The append-only store + `current` pointer are untouched.
- **Why it's the right shape:** it is exactly the spec the model-tiering flow needs — the premium
  orchestrator writes the `WorkOrder`; a cheaper implementer retrieves it (`get_handoff`) and executes
  against `acceptance`; the verifier checks against the same `acceptance`. The contract is explicit and
  falsifiable, which is what keeps a cheap producer from shipping confidently-wrong work.
- **MCP surface:** `tag_handoff` gains optional work-order fields; `get_handoff` returns them. Additive.

## Scope tag (ties to the shared-ledger decision)

Per `ember-smb-platform/design/shared-ledger-scoping.md`, handoffs/work-orders need a **scope**: a
**repo/team work-order is shared** (default), a **personal pause-note is user-scoped**. Add an optional
`scope: { shared | user:<id> }` (default `shared` for a work-order, since the point is hand-off to
another executor; a bare pause-note can stay user-scoped). This is the cortex-side reference for the
shared multi-user ledger model — cortex's eventual fix for team/repo ledgers builds on this.

## How it closes the loop

The `WorkOrder.acceptance` is the **falsifier spec** of the producer/verifier loop, made durable and
retrievable. The spec→outcome (did execution meet `acceptance`?) is the signal that (a) consolidates
into a durable learning and (b) feeds the platform's `ConfidenceOracle`
(`ember-smb-platform` work item #1) — so the orchestrator *learns which work-orders succeed*. That is
the shared loop's memory leg closing on itself.

## Sequencing

1. **(this doc)** decision: spec = structured work-order in the **handoff** tier (state), pattern →
   ledger via episodic; correct the "durable ledgered artifact" wording.
2. Implement `WorkOrder` + `Handoff.work_order` (+ scope) — backward-compatible; update `tag_handoff` /
   `get_handoff` + tests.
3. Wire the producer/verifier flow to read `acceptance` from the retrieved work-order.
4. (cross-repo) feed spec→outcome into `ConfidenceOracle` + episodic consolidation.

Not urgent relative to first-tenant basics, but the `WorkOrder` shape should land before the
orchestrator/implementer split is used heavily, so plans are captured structured from the start.

Related: `docs/episodic-consolidation-spec.md`, `ember-smb-platform/design/shared-loop-architecture.md`,
`ember-smb-platform/design/shared-ledger-scoping.md`, the `cortex-orientation` skill (producer/verifier
+ pattern-vs-state).

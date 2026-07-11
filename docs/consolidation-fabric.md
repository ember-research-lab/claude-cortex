# Consolidation Fabric — cross-surface, cross-repo agent memory

**Status:** design of record. The "why" behind cortex. Extends
`vnext-substrate-spec.md` (which is the substrate); this is the *goal that
substrate serves*. Scope: **org-wide** (all ember-research-lab + whale-signal),
with a **higher rigor bar** for research-adjacent work.

## The problem (stated by Aaron, 2026-07-09)
Agent work fragments across four axes and never consolidates into one coherent,
current state, so **the human becomes the memory bus** between agents:

- **parallel** — one orchestrator per repo (spatially partitioned, single-writer,
  human-steered). Not a merge-conflict problem — a *shared-brain* problem.
- **temporal** — compaction + session boundaries drop the thread.
- **surface** — chat (claude.ai), code (Claude Code), Grok sessions, other
  agents — each a silo.
- **evidential** — a "done" or a "0.999 signal" that a later run contradicts,
  with nothing propagating the contradiction back.

The valuable object is **the thread** — reasoning, decisions, cross-project
connections, insights — which mostly lives in *chats and sessions, not in any
codebase*. Goal experience: *"I talk to you (or Grok) from any repo, even an
unrelated one, and you already understand the thread about another project —
including parts that were never in code."* "AGI structure applicable in context"
= **engineered consolidation**, not model magic. Coherence you build.

## Measured reality (2026-07-09 inventory)
| Surface | Size | In the brain? |
|---|---|---|
| claude.ai chats (ember-chat-search) | **634 conv / 18,908 msgs** | ❌ not connected to cortex |
| cortex GLOBAL ledger | 116 learnings + 13 handoffs | ✅ but fed only from Claude Code |
| Grok sessions | 11, siloed per-directory | ❌ separate agent, separate memory |
| local ledger islands | 2 (umbrella-level) | partial |

**The chat corpus is ~160× the ledger and entirely outside the brain.** We built
spectral/epistemic machinery on the 116-learning *tail* while the 18,908-message
*body of actual thinking* sits unconsolidated. The ledger is the tail; the chats
are the dog. The thread also *wants* to link: the first chat hit for "whale
signal ascent" contains the Sharadar/actor-vs-momentum reasoning **and** says
"Aaron's showing me a Claude Code session" — it references the code session;
nothing links them.

## Honest constraints (platform, not design)
- **Chat capture is manual-export-gated.** ember-chat-search ingests
  `conversations.json` from a downloaded claude.ai data-export `.zip` (a
  "monthly workflow"). claude.ai has **no live API** for your own history;
  share-links are Cloudflare-walled. So *real-time* seamless chat capture is
  platform-blocked. Mitigations: cron the index+consolidate (auto after export);
  browser-automate the export (Playwright/chrome-devtools MCP); or ride the
  existing ember-queue mobile→Drive pipeline.
- **Local surfaces ARE fully auto-capturable now** — Claude Code sessions
  (cortex-episodic) and Grok sessions (local files, per-directory).
- **Mobile/remote agents (grok.com, claude.ai app) can't reach a LOCAL brain** —
  needs a hosted endpoint. Later. The local case covers most of the pain.

## Architecture
```
[Claude Code sessions] ─┐
[Grok CLI sessions]     ├─→ (cron: CHEAP-MODEL entity extractor) ─→ [GLOBAL GRAPH]
[claude.ai export]  ────┘        thread + distill                  entities + threads
                                                                   + contestable confidence
                                                                        │
                                                          cortex-mcp ───┼──→ Claude Code (any repo)
                                                                        └──→ Grok CLI (registered)
```

Everything already built is the **skeleton** of this fabric:
- **ember-graph** = the consolidation *fabric* (entities = nodes; the thread =
  edges). Its real purpose — not learning retrieval.
- **cortex-episodic** = multi-surface capture (extend beyond Claude Code).
- **consolidator** = the distillation that *merges* streams into the graph.
- **epistemic confidence** = why parallel/conflicting inputs resolve by *evidence
  + contestation*, not overwrite. Two agents can assert opposites; both are held
  as claims until one is contested.
- **bi-temporal edges (Phase 4)** = *needed here* for temporal/parallel coherence
  ("what was true when", supersede stale). Not premature — premature *as
  standalone*, correct *in service of consolidation*.
- **progress-as-contestable-claims** and **whale-signal's lucky-seed** are the
  same problem (a claim not reconciled against evidence) — same fabric.

## The heart: the entity spine (the hard core)
An LLM extractor (cheap lane — Grok/Haiku, on the cron) reads new content from
each surface and emits `(entity, canonical_name, mention, evidence)`, resolving
NL variants — "ascent A-signal" ≈ "whale signal ascent" ≈ the "HEAD-SEED-FRAGILE"
finding → **one** content-addressed node (`NodeId::canonical`). Each surface's
contribution attaches to that node. **Entity resolution across NL surfaces is the
genuinely hard part** — it is not deterministic; it is the extractor's judgment.

## Progress = contestable claims (the org-wide protocol)
Kill boolean checklists. A milestone is a claim with: an **origin** (Extracted =
measured/verified vs Inferred = looks-done-untested), **evidence** (a falsifiable
acceptance check — a measurement, a seed-sweep, a token delta — not "it
compiled"), and it is **contestable** (a later Failure flips Validated→Contested
and it stops being load-bearing). "Done" is *Validated-until-contested*, not a
latch. This is the same cortex confidence mechanism, applied to progress. Higher
bar for ember-research/whale-signal: their claims carry the seed-robust/DSR-PBO
evidence discipline before they can be "verified".

## Build sequence
0. **Cross-agent unlock (now):** register `cortex-mcp` in Grok's config (user
   scope) — proven that Grok reads its 13 tools headless. Grok stops being
   thread-blind.
1. **Entity spine:** extractor (cheap lane) → global graph entities; resolve NL
   variants to one node. Prove on the whale-signal thread.
2. **Multi-surface ingest:** thread chat (from export) + Claude Code + Grok
   sessions onto entities. Cron the pass (auto after data lands).
3. **Consolidated read:** "thread on X" from any repo → chat reasoning + ledger
   findings + code, together, contestable. The demo: from claude-cortex,
   answer "what's the thread on whale-signal ascent" with cross-surface content
   not in this repo.
4. **Chat-export automation** (parallel track, platform plumbing): browser-auto
   or ember-queue, to reduce the one manual step.
5. **Later:** hosted endpoint (mobile agents); meta-orchestrator that steers the
   per-repo orchestrators (reads this brain, dispatches) — replacing the human
   router.

## Shipped (status 2026-07-11)
Implemented in [`../scripts/`](../scripts/) (see `scripts/README.md` + `scripts/export-automation/SETUP.md`):
- **Step 0 — cross-agent unlock:** DONE. `cortex-mcp` registered in Grok; Grok reads the brain headless.
- **Step 2 — multi-surface ingest (chat):** DONE (safe form). The producer (`consolidate_run.py`) extracts → **independently provenance-verifies** → contradiction-gates → queues **review items**; a human drains (`cortex_review.py`). Key correction vs the original plan: full-autonomous *commit* is unsafe (contradiction detection is recall-limited), so the commit is human-in-loop while the tedium is automated. Runs on the Claude subscription.
- **Step 4 — chat-export automation:** DONE + proven end-to-end. Standalone Playwright (headed to pass Cloudflare, cookie auth) requests + downloads the export; Gmail-IMAP retrieves the link; a new zip auto-indexes and triggers the producer.
- **Still open:** Step 1 (entity spine — currently flat `[chat]` tags + topic-neighbor annotation, not yet a resolved-entity graph), Step 3 (consolidated cross-surface read), Step 5 (hosted endpoint / meta-orchestrator).

## Success test (falsifiable, not "it compiled")
From a repo unrelated to whale-signal, ask an agent (Claude **or** Grok) "what's
the thread on the whale-signal ascent signal?" and get: the chat-side reasoning
(Sharadar/survivorship, actor-vs-momentum, spectral-GNN-vs-RDT) **plus** the
code-side findings (HEAD-SEED-FRAGILE, the lucky-seed retraction) **plus** the
current contestable status — consolidated, with zero re-explaining. Today that
returns nothing cross-surface. That gap closing is the whole point.

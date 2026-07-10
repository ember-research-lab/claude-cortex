# Consolidation fabric — automation scripts

Cross-surface chat consolidation for the cortex ledger, done **safely** (producer
extracts + verifies autonomously; a human drains a review queue for the one
recall-limited judgment call). See [`../docs/consolidation-fabric.md`](../docs/consolidation-fabric.md)
for the "why".

## Flow

```
cortex-chat-consolidate.timer   (cheap freshness heartbeat; zero cost on quiet days)
        │
        ▼
chat-consolidate.sh  ──►  consolidate_run.py "<scope>" --review
        │                       │  1. EXTRACT   (claude -p on the SUB + ember-chat-search)
        │                       │  2. VERIFY    (independent claude -p re-checks provenance, by cid)
        │                       │  3. GATE      (dedup + contradiction classify + neighbor-annotate)
        │                       ▼
        │                 REVIEW QUEUE  (~/.local/state/cortex-consolidate/review/pending/*.json)
        │                       │  nothing is written to the ledger autonomously
        ▼                       ▼
   marker advances       cortex_review.py list / approve / reject / approve-clean
                                │  approvals commit via cortex_client.py (reliable direct MCP)
                                ▼
                          cortex ledger
```

## Scripts

| Script | Role |
|---|---|
| `chat-consolidate.sh` | Freshness-gated heartbeat that fills the review queue. Runs on the **Claude subscription** (`env -u ANTHROPIC_API_KEY`), not the metered API. |
| `consolidate_run.py` | The producer: extract → independent verify → contradiction gate → neighbor annotate. `--review` (queue, default for autonomy), `--dry` (no writes), else direct-write. |
| `cortex_review.py` | Drain the review queue: `list` (conflicts + neighbors first), `approve <id> [--force-tag]`, `reject <id>`, `approve-clean` (only truly-novel items). |
| `cortex_client.py` | Reliable direct cortex-mcp stdio client — the write path. Sidesteps the headless plugin-MCP connection race; a fresh process connects in ~0.02s. |
| `ledger_count.py` | Independent `total_learnings` count via the public MCP interface (format-stable). |

## Safety model

- **Nothing is tagged autonomously.** The producer only queues review items.
- **Provenance verify** (stage 2) is an independent pass that re-checks each claim's
  evidence quote against the corpus — catches overstatement/contradiction of the source.
- **Contradiction gate + neighbors** (stage 3): the gate is *recall-limited* (a search may
  miss a conflicting prior claim), so every item also lists genuinely topic-adjacent
  existing learnings (≥3 shared terms). `approve-clean` bulk-commits only items with **no**
  neighbors; anything topic-adjacent needs human eyes.

## Setup

```sh
# install the daily heartbeat (fills the queue; no-op at zero cost when no new export)
cp scripts/systemd/cortex-chat-consolidate.* ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now cortex-chat-consolidate.timer

# optional overrides in ~/.cortex-consolidate.env:
#   CORTEX_CONSOLIDATE_MODEL=sonnet     # driver model
#   CORTEX_CONSOLIDATE_NTFY=<topic>     # push a summary on new review items
```

Drain whenever: `python3 scripts/cortex_review.py list`.

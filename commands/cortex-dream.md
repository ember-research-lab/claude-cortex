---
description: Run a cortex dreaming pass — index the ledger, build the spectral graph, decompose, and emit an active-memory snapshot for the next session to read from.
allowed-tools: Bash
---

# /cortex-dream

Trigger a cortex v4 dreaming pass against your project ledger.

## What it does

1. Reads every learning from `<project>/.claude/cortex/ledger/`.
2. Builds a BM25 index over the learning content (no external API; fully offline).
3. Constructs the learning graph: edges weighted by `0.4·BM25_similarity + 0.4·co_occurrence + 0.2·outcome_correlation`. Co-occurrence and outcome signals are passed in empty in v4.0 (lands in v4 minor releases).
4. Computes the top-`k` eigendecomposition of the graph Laplacian. Default `k = min(50, n/3)`.
5. Writes an active-memory snapshot to `<project>/.claude/cortex/ledger/cortex-state/active/active-{ts}.json` and advances the `current` pointer.
6. Records a spectrum snapshot under `<project>/.claude/cortex/ledger/cortex-state/spectrum-history/snapshot-{ts}.json` for cortex-monitor.

## When to run

Manually, when you want cortex to refresh its working set after a productive period of new learnings. The v4.0 dreaming trigger is manual-only by intent — automatic cron/session-end triggers are deferred until usage data shows dreaming actually adds value.

## Performance

- Ledgers under 500 entries: dense eigendecomposition, typically <1 second.
- Ledgers 500–10k: budget is <60s; v4.0 still uses the dense path which becomes O(n³). Lanczos integration is the planned upgrade for larger ledgers.

## Usage

```sh
/cortex-dream
```

Or with explicit paths / `k` override:

```bash
cortex-dream --ledger /path/to/ledger --state /path/to/state --k 30
```

The slash command runs the binary against the project ledger at the current working directory by default.

---

!`cortex-dream --ledger "${PWD}/.claude/cortex/ledger" --state "${PWD}/.claude/cortex/ledger/cortex-state"`

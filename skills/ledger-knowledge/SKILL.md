---
name: ledger-knowledge
description: Query the cortex knowledge ledger for prior learnings, patterns, and decisions. Use the MCP tools first; CLI is a fallback. Triggers on "list learnings", "show learning", "check the ledger", "what do we know about", "search learnings", "find in ledger", "search for pattern", "what's stored", "lookup", "fetch from ledger", "ledger stats", "how many learnings".
version: 0.3.4
---

# Ledger Knowledge

Read access to the cortex ledger via the cortex MCP tools.

## Locations

- **Global**: `~/.claude/ledger/` — knowledge shared across projects
- **Project**: `<project>/.claude/cortex/ledger/` — project-specific knowledge

(v2 used `<project>/.claude/ledger/`; v3 moves to `cortex/ledger/`. If a v2 layout is detected, run `cortex-migrate` per the orientation skill.)

## Preferred: MCP tools

```
search_learnings(query, category?, min_confidence?, limit?, project_dir?)
get_learning(learning_id, show_outcomes?, show_decay?, project_dir?)
list_learnings(min_confidence?, category?, limit?, show_decay?, project_dir?)
ledger_stats(project_dir?)
```

`learning_id` accepts an 8-char prefix or the full UUID. `project_dir` defaults to the global ledger; pass an absolute path for a project ledger.

## Categories

| Category   | Use for                                                       |
|------------|---------------------------------------------------------------|
| discovery  | New information about a codebase, API, or environment         |
| decision   | Architectural / design choices with rationale                 |
| error      | Mistakes to avoid, gotchas, footguns                          |
| pattern    | Reusable conventions, templates, idioms                       |

## Confidence interpretation

- 0.85+ — very high; apply by default unless the situation contradicts
- 0.65-0.85 — strong; apply with light verification
- 0.50-0.65 — hedged; use as a hint, verify before acting
- <0.50 — unverified; treat as a suggestion

Effective confidence (with 180-day exponential decay) is what `show_decay: true` returns. Stored confidence is the un-decayed value; effective is what you should weight decisions by.

## Examples

```
search_learnings("atomic writes", category="pattern", min_confidence=0.7)
get_learning("6d3ff6f0", show_outcomes=true, show_decay=true)
list_learnings(min_confidence=0.8, limit=10)
ledger_stats()
```

## Recording outcomes

Once a learning has been *exercised* (looked up + applied), call `record_outcome` with success / partial / failure and a one-line context. This is how confidence converges to reality. The outcome-recorder agent does this automatically at session end; the orchestrator can also call directly.

---
name: learning-capture
description: Capture insights to the cortex ledger via the tag_learning MCP tool. Use for discoveries, decisions, errors, and patterns worth remembering for the next session. Triggers on "tag this", "remember this", "save learning", "capture this", "worth remembering", "store this insight", "log this discovery", "note this for next time", "add to ledger", "persist this".
version: 0.3.6
---

# Learning Capture

Persist insights to the cortex ledger so the next session benefits.

## When to capture

| Category   | When to use                                                   |
|------------|---------------------------------------------------------------|
| discovery  | Found out how a system, API, or codebase actually works       |
| decision   | Made an architectural / approach choice with reasoning        |
| error      | Hit a mistake / gotcha / footgun worth flagging               |
| pattern    | Identified a reusable convention or template                  |

## How to capture

```
tag_learning(
  content="<one-sentence insight, max 500 chars>",
  category="discovery" | "decision" | "error" | "pattern",
  confidence=0.7,             # default — typical for fresh insights
  source_file="path/to/file", # optional — where it was discovered
  project_dir="<absolute>",   # default null = global ledger
)
```

## Confidence guidelines for new learnings

- 0.8 — directly observed and validated in the current session
- 0.7 — observed and likely correct (default)
- 0.6 — inferred from context, plausible but unverified
- 0.5 — speculative; capture for future sessions to confirm/refute

Confidence updates based on subsequent `record_outcome` calls. Pick a reasonable starting value; let outcomes do the rest.

## What to capture

- Non-obvious facts a future Claude session would benefit from knowing without re-running the tool that revealed them.
- Decisions where the *reasoning* matters — if removing the rationale would make the choice look arbitrary, capture both decision and reasoning.
- Errors with specific reproducer-style detail. "Don't do X with Y because Z fails" beats "Y is fragile."

## What NOT to capture

- Routine outputs from `Read`, `Glob`, `Grep`, etc.
- Restating what's already in the codebase. The code is the source of truth; capture meta-knowledge about *how to navigate* the code.
- One-off task state. Use TaskCreate / handoff for in-flight session state.

## Anti-pattern: inline tags

Do NOT inline `[DISCOVERY] ...` / `[DECISION] ...` / `[ERROR] ...` / `[PATTERN] ...` markers in conversation text. That was the v2 pattern; v3 captures via `tag_learning` directly so the ledger always reflects what was actually persisted.

---
name: handoff
description: Record a session handoff at a pause-point — captures completed/pending tasks, blockers, modified files, and free-form context so the next session can resume cleanly.
---

# /handoff

Record a handoff capturing the current state of work. Handoffs live in the ephemeral state layer (NOT the long-term ledger) — they're meant for the kind of "where did I leave off" details that go stale within days.

## Action

Call the `tag_handoff` MCP tool with the current session id and the user-provided fields. If the user invokes `/handoff` with no specifics, gather:

- **completed_tasks** — what shipped this session (one per line is fine)
- **pending_tasks** — what's still open
- **blockers** — anything waiting on the user, an external system, or a decision
- **modified_files** — paths touched (paste from `git status` if relevant)
- **context_notes** — where exactly you paused, what the next session should know

If the user provides a single block of text, parse it into the structured fields rather than dumping everything into `context_notes`.

After recording, briefly confirm where the handoff was stored and when. The next session's `get_handoff` call will surface it automatically.

## When NOT to use

- For durable patterns or framework-level discoveries → those go in the ledger via `tag_learning`.
- For trivial pauses (one-line context, no work in flight) → no need; an empty handoff just adds noise.

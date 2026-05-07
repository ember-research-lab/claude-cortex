---
name: handoff-management
description: Save and load work-in-progress state for session continuity. Distinct from learnings (permanent knowledge); handoffs are ephemeral session state. Triggers on "save progress", "create handoff", "show handoff", "what was I working on", "handoff", "session handoff", "resume work", "where did we leave off", "pick up where I left off".
version: 0.3.2
---

# Handoff Management

Handoffs capture work-in-progress state — what's done, what's pending, what's blocking — for session continuity. Distinct from learnings: learnings are permanent knowledge; handoffs are ephemeral state.

## v3 status

v3 ships with the substrate (ledger, signing, MCP tool surface for the seven ledger-grounded tools). The handoff store is **scheduled but not yet ported**; the `get_handoff` MCP tool currently returns a `pending v3.x` notice. Continue to use TaskCreate / TaskUpdate within a session for in-flight state, and rely on the session_start hook (which surfaces top learnings, not in-flight tasks) plus the user's own notes for cross-session continuity.

When the handoff substrate ships, this skill will be updated with the actual call surface. The fields are stable across the planned port:

```
get_handoff(session_id?, project_dir?) -> {
  session_id, timestamp,
  completed: [...],
  pending_tasks: [...],
  blockers: [...],
  context: "..."
}
```

## In the meantime

- Use the orchestrator's TaskCreate / TaskUpdate for in-session state.
- For cross-session continuity, the session_start hook surfaces top learnings (the persistent layer). Capture in-flight context as a learning if it's likely to outlast the immediate session.
- The auto-memory MEMORY.md system at `~/.claude/projects/.../memory/MEMORY.md` provides cross-session project state today; handoffs will complement it once ported.

## Anti-patterns

- Don't capture handoff content as learnings just because handoffs aren't available. Ephemeral task state in the ledger pollutes the long-term knowledge surface.
- Don't reach into v2 handoff files manually. The on-disk format is changing in the v3 port.

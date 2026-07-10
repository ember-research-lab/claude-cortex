#!/usr/bin/env python3
"""Drain the native cortex consolidation REVIEW QUEUE — the one human/architect step.

The autonomous producer (consolidate_run.py --review) NEVER writes to the ledger; it queues
review items. Approving commits via the reliable direct MCP client; rejecting discards. This
keeps judgment in the loop for the recall-limited contradiction call, while automating the
rest. Native to the fabric — no dependency on the (superseded, buggy) ember-queue.

  cortex_review.py list                 # pending items, newest concerns first
  cortex_review.py show <id>
  cortex_review.py approve <id> [--force-tag]   # commit proposed action; contradicts need --force-tag
  cortex_review.py reject <id>
  cortex_review.py approve-clean        # commit every non-conflict 'tag' item at once
"""
import glob
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cortex_client import cortex_session  # noqa: E402

STATE_DIR = os.environ.get("CORTEX_CONSOLIDATE_STATE",
                           os.path.expanduser("~/.local/state/cortex-consolidate"))
PENDING = os.path.join(STATE_DIR, "review", "pending")
APPROVED = os.path.join(STATE_DIR, "review", "approved")
REJECTED = os.path.join(STATE_DIR, "review", "rejected")


def _items(d):
    return sorted(glob.glob(os.path.join(d, "*.json")))


def _load(p):
    with open(p) as f:
        return json.load(f)


def _find(item_id):
    p = os.path.join(PENDING, f"{item_id}.json")
    return p if os.path.exists(p) else None


def _move(src, dest_dir, extra):
    os.makedirs(dest_dir, exist_ok=True)
    item = _load(src)
    item.update(extra)
    dest = os.path.join(dest_dir, os.path.basename(src))
    tmp = dest + ".tmp"
    with open(tmp, "w") as f:
        json.dump(item, f, indent=2)
    os.replace(tmp, dest)
    os.remove(src)
    return dest


def cmd_list(_):
    items = _items(PENDING)
    if not items:
        print("(review queue empty)")
        return 0
    # contradictions first — they need attention
    rows = [_load(p) for p in items]
    # order: contradictions first, then items with neighbors (potential conflict), then clean-novel
    rows.sort(key=lambda it: (it.get("proposed_action") != "contradict", not it.get("neighbors")))
    for it in rows:
        act = it.get("proposed_action", "tag")
        nb = it.get("neighbors", [])
        flag = ("⚠ CONTRADICT" if act == "contradict"
                else f"⚑ {len(nb)} neighbor(s)" if nb else "✓ novel")
        print(f"{it['id']}  [{flag}]  {it['content'][:72]}")
        for n in nb[:3]:  # topic-adjacent existing learnings — spot conflicts the gate missed
            print(f"        ~ {n['id']} (c={n.get('confidence')}): {n.get('snippet','')[:72]}")
    clean = sum(1 for it in rows if not it.get("neighbors") and it.get("proposed_action") == "tag")
    print(f"\n{len(items)} pending ({clean} clean-novel, {len(items)-clean} need eyes). "
          f"approve <id> [--force-tag] / reject <id> / approve-clean")
    return 0


def cmd_show(args):
    p = _find(args[0])
    if not p:
        print(f"not found: {args[0]}", file=sys.stderr)
        return 1
    print(json.dumps(_load(p), indent=2))
    return 0


def _commit(item, force_tag):
    action = item.get("proposed_action", "tag")
    with cortex_session() as call:
        if action == "corroborate":
            call("record_corroboration", {"learning_id": item["related_learning_id"],
                                          "context": f"[chat] re-observed: {item['content'][:120]}"})
            return {"committed": "corroborate", "learning_id": item["related_learning_id"]}
        if action == "contradict" and not force_tag:
            return None  # signal: needs explicit resolution
        content = item["content"] if item["content"].startswith("[chat]") else "[chat] " + item["content"]
        r = call("tag_learning", {"content": content[:500],
                                  "category": item.get("category", "discovery"),
                                  "confidence": float(item.get("confidence", 0.6))})
        return {"committed": "tag", "learning_id": r.get("learning_id")}


def cmd_approve(args):
    force = "--force-tag" in args
    ids = [a for a in args if not a.startswith("--")]
    if not ids:
        print("usage: approve <id> [--force-tag]", file=sys.stderr)
        return 2
    p = _find(ids[0])
    if not p:
        print(f"not found: {ids[0]}", file=sys.stderr)
        return 1
    item = _load(p)
    res = _commit(item, force)
    if res is None:
        print(f"{ids[0]} is a CONTRADICTION vs {item['related_learning_id']}. Resolve explicitly:\n"
              f"  approve {ids[0]} --force-tag   (record as a new, competing claim)\n"
              f"  reject {ids[0]}                (drop it)")
        return 2
    _move(p, APPROVED, res)
    print(f"approved {ids[0]}: {res}")
    return 0


def cmd_reject(args):
    p = _find(args[0])
    if not p:
        print(f"not found: {args[0]}", file=sys.stderr)
        return 1
    _move(p, REJECTED, {"committed": "rejected"})
    print(f"rejected {args[0]}")
    return 0


def cmd_approve_clean(_):
    # Only truly-novel items (no topic-adjacent neighbor) are safe to bulk-commit — anything
    # with a neighbor could be a conflict the recall-limited gate missed (#13). Those need eyes.
    n, skipped = 0, 0
    for p in _items(PENDING):
        it = _load(p)
        if it.get("proposed_action") == "tag" and not it.get("neighbors"):
            res = _commit(it, False)
            if res is None:
                continue
            _move(p, APPROVED, res)
            print(f"approved {it['id']}: {res['learning_id']}")
            n += 1
        else:
            skipped += 1
    print(f"approve-clean: committed {n} clean-novel item(s); left {skipped} with neighbors/conflicts for manual review.")
    return 0


CMDS = {"list": cmd_list, "show": cmd_show, "approve": cmd_approve,
        "reject": cmd_reject, "approve-clean": cmd_approve_clean}


def main(argv):
    if len(argv) < 2 or argv[1] not in CMDS:
        print(__doc__)
        return 2
    return CMDS[argv[1]](argv[2:])


if __name__ == "__main__":
    sys.exit(main(sys.argv))

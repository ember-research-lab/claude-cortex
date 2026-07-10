#!/usr/bin/env python3
"""Autonomous chat-consolidation pipeline — safe full autonomy.

Stages (all on the Claude subscription; env -u ANTHROPIC_API_KEY):
  1. EXTRACT  — claude -p + ember-chat-search -> JSON candidates, each with a VERBATIM
                evidence_quote. We stamp a stable `cid` on each (never trust model ordering).
  2. VERIFY   — an INDEPENDENT claude -p re-checks each candidate against the corpus and
                echoes the cid: does the quote EXIST and SUPPORT the claim (not overstate/
                contradict)? Aligned by cid, not array index. PASS-only survive.
  3. GATE     — for each survivor, the reliable direct-MCP client finds a same-topic existing
                learning; a conflict classifier labels the relation CORROBORATE / CONTRADICT /
                DISTINCT. CONTRADICTs are NEVER written — they go to a review file + ntfy
                (the chat-consolidator's original job). CORROBORATE -> record_corroboration.
                DISTINCT / no-match -> tag_learning. Independent ledger_stats delta is asserted.

No item is tagged without (a) an independent PASS and (b) a non-CONTRADICT relation. Usage:
    python3 consolidate_run.py "<scope>" [--dry]
"""
import hashlib
import json
import os
import re
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from cortex_client import cortex_session  # noqa: E402

SUB_ENV = {k: v for k, v in os.environ.items() if k != "ANTHROPIC_API_KEY"}
MODEL = os.environ.get("CORTEX_CONSOLIDATE_MODEL", "sonnet")
STATE_DIR = os.environ.get("CORTEX_CONSOLIDATE_STATE",
                           os.path.expanduser("~/.local/state/cortex-consolidate"))
REVIEW_PENDING = os.path.join(STATE_DIR, "review", "pending")
NTFY = os.environ.get("CORTEX_CONSOLIDATE_NTFY", "")


def emit_review_item(item):
    """Atomically drop a review item into the native fabric review queue (pending)."""
    os.makedirs(REVIEW_PENDING, exist_ok=True)
    rid = hashlib.sha256((item["content"] + item.get("scope", "")).encode()).hexdigest()[:12]
    item["id"] = rid
    path = os.path.join(REVIEW_PENDING, f"{rid}.json")
    if os.path.exists(path):
        return rid  # idempotent — already queued
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(item, f, indent=2)
    os.replace(tmp, path)
    return rid


def claude_json(prompt, timeout=600):
    p = subprocess.run(
        ["claude", "-p", "--dangerously-skip-permissions",
         "--model", MODEL, "--output-format", "json", prompt],
        capture_output=True, text=True, env=SUB_ENV, timeout=timeout,
    )
    try:
        return json.loads(p.stdout).get("result", "")
    except json.JSONDecodeError:
        return p.stdout


def parse_json_array(text):
    m = re.search(r"\[.*\]", text or "", re.S)
    if not m:
        return []
    try:
        return json.loads(m.group(0))
    except json.JSONDecodeError:
        return []


def sig_words(s):
    return {w for w in re.findall(r"[a-z0-9]{4,}", (s or "").lower())}


def ntfy(msg):
    if not NTFY:
        return
    try:
        subprocess.run(["curl", "-fsS", "-d", msg, f"https://ntfy.sh/{NTFY}"],
                       capture_output=True, timeout=15)
    except Exception:
        pass


EXTRACT_PROMPT = """You are a cross-surface consolidation EXTRACTOR. Use \
mcp__ember-chat-search__search_conversations (context_turns=2) on the scope "{scope}" and \
related salient terms; read the surrounding turns, not just snippets.

Extract ONLY DURABLE thread items (true in a year): decisions WITH reasoning, cross-project \
connections, non-obvious insights. DROP transient state and generic facts. For EACH item output:
  "content": self-contained, <=450 chars, no session references
  "category": discovery|decision|error|pattern
  "confidence": 0.6
  "evidence_quote": a VERBATIM span (<=200 chars) copied EXACTLY from a real chat turn supporting the claim
  "conversation_title": source conversation title
  "dedup_query": 3-6 key terms
Output ONLY a JSON array (no prose/fence). If nothing durable, output [].
"""

VERIFY_PROMPT = """You are an INDEPENDENT PROVENANCE VERIFIER. Do NOT trust the candidates; \
re-derive. For each candidate, use mcp__ember-chat-search__search_conversations to locate its \
evidence_quote, then judge STRICTLY:
  - EXISTS: does a distinctive span of the quote actually appear in the corpus?
  - SUPPORTS: does the quote support the claim WITHOUT overstating/contradicting it? (claim "chose X" \
but quote "rejected X" => DROP; "collapses to zero" but quote "compresses" => DROP.)
Output ONLY a JSON array of {{"cid": "<echo the candidate's cid>", "verdict": "PASS"|"DROP", \
"reason": "<short>"}}. PASS only if EXISTS and SUPPORTS. Candidates:
{candidates}
"""

CONFLICT_PROMPT = """You classify how a NEW claim relates to an EXISTING ledger learning on a \
similar topic. For each pair output the relation:
  - CORROBORATE: same fact/decision; the new observation reinforces it.
  - CONTRADICT: logically incompatible (e.g. new "chose X" vs existing "rejected X"; opposite verdict/number).
  - DISTINCT: related topic but a different, non-conflicting fact.
Output ONLY a JSON array [{{"cid":"<cid>","relation":"CORROBORATE|CONTRADICT|DISTINCT"}}]. Pairs:
{pairs}
"""


def main():
    if len(sys.argv) < 2:
        print("usage: consolidate_run.py <scope> [--dry]", file=sys.stderr)
        return 2
    scope = sys.argv[1]
    dry = "--dry" in sys.argv[2:]
    review = "--review" in sys.argv[2:]  # emit review items instead of writing to the ledger
    os.makedirs(STATE_DIR, exist_ok=True)
    stamp = time.strftime("%Y%m%dT%H%M%S")

    # 1. EXTRACT (stamp our own stable cids)
    candidates = parse_json_array(claude_json(EXTRACT_PROMPT.format(scope=scope)))
    for i, c in enumerate(candidates):
        c["cid"] = f"c{i}"
    print(f"EXTRACT: {len(candidates)} candidate(s)")
    if not candidates:
        print("nothing durable to consolidate.")
        return 0

    # 2. VERIFY (independent, aligned by cid)
    payload = json.dumps([{"cid": c["cid"], "content": c["content"],
                           "evidence_quote": c.get("evidence_quote", "")} for c in candidates])
    vmap = {str(v.get("cid")): v for v in parse_json_array(claude_json(VERIFY_PROMPT.format(candidates=payload)))
            if isinstance(v, dict)}
    passed = []
    for c in candidates:
        v = vmap.get(c["cid"], {})
        c["_verdict"] = str(v.get("verdict", "")).upper()
        c["_vreason"] = v.get("reason", "(no verdict for cid -> drop)")
        ok = c["_verdict"] == "PASS"
        print(f"  [{c['cid']}] {'PASS' if ok else 'DROP'}: {c.get('content','')[:64]}...  ({c['_vreason']})")
        if ok:
            passed.append(c)
    print(f"VERIFY: {len(passed)}/{len(candidates)} passed provenance")

    # 3. GATE (dedup + contradiction) + WRITE
    contradictions, tagged, corroborated = [], [], []
    with cortex_session() as call:
        # find same-topic existing learning per survivor
        pairs = []
        for c in passed:
            q = c.get("dedup_query") or c["content"][:80]
            hits = (call("search_learnings", {"query": q, "limit": 5}) or {}).get("results", [])
            cw = sig_words(c["content"])
            # NEIGHBORS: genuinely topic-adjacent existing learnings (>=3 shared significant
            # terms — BM25 alone returns hits for any query, so filter for real overlap) so the
            # human can spot conflicts the recall-limited contradict gate missed (#13).
            c["_neighbors"] = [{"id": h["id"], "snippet": h.get("snippet", "")[:110],
                                "confidence": h.get("confidence")} for h in hits
                               if len(cw & sig_words(h.get("snippet", ""))) >= 3]
            top = next((h for h in hits
                        if cw and len(cw & sig_words(h.get("snippet", ""))) / max(1, len(cw)) >= 0.5), None)
            c["_dup_id"] = top["id"] if top else None
            if top:
                full = call("get_learning", {"learning_id": top["id"]}) or {}
                pairs.append({"cid": c["cid"], "new": c["content"],
                              "existing": full.get("content", top.get("snippet", ""))})
        relmap = {}
        if pairs:
            relmap = {str(r.get("cid")): str(r.get("relation", "")).upper()
                      for r in parse_json_array(claude_json(CONFLICT_PROMPT.format(pairs=json.dumps(pairs))))
                      if isinstance(r, dict)}
        for c in passed:
            c["_relation"] = relmap.get(c["cid"], "DISTINCT")

        audit = {"scope": scope, "stamp": stamp, "candidates": candidates}
        with open(os.path.join(STATE_DIR, f"audit-{stamp}.json"), "w") as f:
            json.dump(audit, f, indent=2)

        if dry:
            for c in passed:
                print(f"  WOULD {'CONTRADICT->review' if (c['_dup_id'] and c['_relation']=='CONTRADICT') else ('CORROBORATE '+c['_dup_id'] if (c['_dup_id'] and c['_relation']=='CORROBORATE') else 'TAG')}: {c['content'][:60]}")
            print("(dry run — no writes)")
            return 0

        if review:
            # Native review queue: NOTHING is committed autonomously. Every survivor
            # becomes a review item; the human/architect drains via cortex_review.py.
            n_conf = 0
            for c in passed:
                proposed = ("corroborate" if (c["_dup_id"] and c["_relation"] == "CORROBORATE")
                            else "contradict" if (c["_dup_id"] and c["_relation"] == "CONTRADICT")
                            else "tag")
                n_conf += proposed == "contradict"
                emit_review_item({
                    "scope": scope, "created": stamp,
                    "content": c["content"], "category": c.get("category", "discovery"),
                    "confidence": float(c.get("confidence", 0.6)),
                    "evidence_quote": c.get("evidence_quote", ""),
                    "conversation_title": c.get("conversation_title", ""),
                    "verdict_reason": c.get("_vreason", ""),
                    "proposed_action": proposed,
                    "related_learning_id": c["_dup_id"],
                    "neighbors": c.get("_neighbors", []),
                })
            print(f"REVIEW-QUEUE: {len(passed)} item(s) -> {REVIEW_PENDING} "
                  f"({n_conf} flagged contradict). Drain with: cortex_review.py list")
            if NTFY:
                ntfy(f"cortex consolidate: {len(passed)} review item(s) queued ({n_conf} contradiction-flagged)")
            return 0

        before = int(call("ledger_stats").get("total_learnings", 0))
        for c in passed:
            if c["_dup_id"] and c["_relation"] == "CONTRADICT":
                contradictions.append({"new": c["content"], "existing_id": c["_dup_id"],
                                       "quote": c.get("evidence_quote", "")})
                continue
            if c["_dup_id"] and c["_relation"] == "CORROBORATE":
                call("record_corroboration", {"learning_id": c["_dup_id"],
                                              "context": f"[chat] re-observed: {c['content'][:120]}"})
                corroborated.append(c["_dup_id"])
                continue
            content = c["content"] if c["content"].startswith("[chat]") else "[chat] " + c["content"]
            r = call("tag_learning", {"content": content[:500],
                                      "category": c.get("category", "discovery"),
                                      "confidence": float(c.get("confidence", 0.6))})
            tagged.append(r.get("learning_id"))
        after = int(call("ledger_stats").get("total_learnings", 0))

    # contradictions -> review file + ntfy (NEVER auto-written)
    if contradictions:
        rf = os.path.join(STATE_DIR, f"contradictions-{stamp}.md")
        with open(rf, "w") as f:
            f.write(f"# Cross-surface CONTRADICTIONS (scope: {scope}) — human review\n\n")
            for x in contradictions:
                f.write(f"- NEW (chat): {x['new']}\n  vs EXISTING ledger {x['existing_id']}\n"
                        f"  chat evidence: \"{x['quote']}\"\n\n")
        print(f"CONTRADICTIONS: {len(contradictions)} -> {rf} (NOT auto-written)")
        ntfy(f"cortex consolidate: {len(contradictions)} CONTRADICTION(s) need review\n{rf}")

    delta = after - before
    print(f"WRITE: tagged={len(tagged)} {tagged} | corroborated={len(corroborated)} {corroborated} "
          f"| contradictions={len(contradictions)}")
    ok = delta == len(tagged)
    print(f"VERIFY-DELTA: ledger {before}->{after} (delta={delta}) vs tags={len(tagged)} => {'OK' if ok else 'MISMATCH!'}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())

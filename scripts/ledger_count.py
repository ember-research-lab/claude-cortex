#!/usr/bin/env python3
"""Print global-ledger total_learnings via a cortex-mcp `ledger_stats` call.

Uses the public MCP (stdio JSON-RPC) interface — NOT the on-disk ledger layout —
so it stays correct across substrate format versions. Used by chat-consolidate.sh
for independent write-verification (the count the agent's self-report is checked
against). Exit 0 + prints an integer on success; exit 1 on failure.
"""
import json
import subprocess
import sys

def _send(proc, msg):
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()

def _read_id(proc, want_id, limit=200):
    for _ in range(limit):
        line = proc.stdout.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            continue
        try:
            m = json.loads(line)
        except json.JSONDecodeError:
            continue
        if m.get("id") == want_id:
            return m
    return None

def main():
    proc = subprocess.Popen(
        ["cortex-mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        text=True, bufsize=1,
    )
    try:
        _send(proc, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                     "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                                "clientInfo": {"name": "ledger_count", "version": "1"}}})
        if _read_id(proc, 1) is None:
            print("ERR: no initialize response", file=sys.stderr); return 1
        _send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})
        _send(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                     "params": {"name": "ledger_stats", "arguments": {}}})
        resp = _read_id(proc, 2)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()

    if not resp:
        print("ERR: no ledger_stats response", file=sys.stderr); return 1
    try:
        text = resp["result"]["content"][0]["text"]
        print(int(json.loads(text)["total_learnings"]))
        return 0
    except (KeyError, IndexError, ValueError, TypeError) as e:
        print(f"ERR: unexpected response shape: {e}: {resp}", file=sys.stderr); return 1

if __name__ == "__main__":
    sys.exit(main())

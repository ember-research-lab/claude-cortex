#!/usr/bin/env python3
"""Direct cortex-mcp stdio client — the RELIABLE ledger read/write path.

Sidesteps the headless plugin-MCP connection race (see learning 86565c47): a
fresh cortex-mcp process completes the MCP handshake and answers in ~0.02s, so
the autonomous consolidation pipeline writes through THIS instead of relying on
a subagent's not-yet-connected plugin tools.

Importable (cortex_session context manager) and CLI:
    python3 cortex_client.py ledger_stats
    python3 cortex_client.py search_learnings '{"query":"whale-signal","limit":2}'
    python3 cortex_client.py tag_learning '{"content":"...","category":"pattern","confidence":0.6}'
"""
import json
import subprocess
import sys
from contextlib import contextmanager


class CortexError(RuntimeError):
    pass


@contextmanager
def cortex_session(bin_path="cortex-mcp"):
    proc = subprocess.Popen(
        [bin_path], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1
    )
    assert proc.stdin is not None and proc.stdout is not None  # PIPE guarantees these
    stdin, stdout = proc.stdin, proc.stdout
    counter = [1]  # id 1 is used by initialize

    def _send(msg):
        stdin.write(json.dumps(msg) + "\n")
        stdin.flush()

    def _read(want_id):
        for _ in range(500):
            line = stdout.readline()
            if not line:
                raise CortexError("cortex-mcp closed stdout")
            line = line.strip()
            if not line:
                continue
            try:
                m = json.loads(line)
            except json.JSONDecodeError:
                continue
            if m.get("id") == want_id:
                return m
        raise CortexError(f"no response for id {want_id}")

    _send({"jsonrpc": "2.0", "id": 1, "method": "initialize",
           "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                      "clientInfo": {"name": "cortex_client", "version": "1"}}})
    _read(1)
    _send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def call(name, args=None):
        counter[0] += 1
        rid = counter[0]
        _send({"jsonrpc": "2.0", "id": rid, "method": "tools/call",
               "params": {"name": name, "arguments": args or {}}})
        resp = _read(rid)
        if "error" in resp:
            raise CortexError(resp["error"])
        try:
            return json.loads(resp["result"]["content"][0]["text"])
        except (KeyError, IndexError, ValueError, TypeError):
            return resp.get("result")

    try:
        yield call
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


def main(argv):
    if len(argv) < 2:
        print("usage: cortex_client.py <tool> [json-args]", file=sys.stderr)
        return 2
    tool = argv[1]
    try:
        args = json.loads(argv[2]) if len(argv) > 2 else {}
    except json.JSONDecodeError as e:
        print(f"bad json args: {e}", file=sys.stderr)
        return 2
    with cortex_session() as call:
        print(json.dumps(call(tool, args)))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

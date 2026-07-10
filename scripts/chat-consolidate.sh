#!/usr/bin/env bash
# chat-consolidate.sh — cheap freshness heartbeat that FILLS the review queue.
#
# Part of the consolidation fabric (docs/consolidation-fabric.md). This does NOT
# write to the ledger. It runs the producer (consolidate_run.py --review), which
# extracts + independently provenance-verifies + contradiction-gates chat-corpus
# deltas and drops the survivors as REVIEW ITEMS. A human/architect drains them
# with cortex_review.py (approve/reject) — judgment stays in the loop for the
# recall-limited contradiction call; the tedium is automated.
#
# Runs on the CLAUDE SUBSCRIPTION (consolidate_run.py strips ANTHROPIC_API_KEY),
# and is FRESHNESS-gated so it costs nothing on days with no new chat export.
# Invoked by cortex-chat-consolidate.timer. Manual: bash chat-consolidate.sh [--force]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INDEX_PATH="${CORTEX_CHAT_INDEX:-$HOME/.local/share/ember-chat-search}"
STATE_DIR="${CORTEX_CONSOLIDATE_STATE:-$HOME/.local/state/cortex-consolidate}"
MARKER="$STATE_DIR/last-run"
LOG="$STATE_DIR/consolidate.log"
FORCE="${1:-}"
export CORTEX_CONSOLIDATE_STATE="$STATE_DIR"   # consolidate_run.py + cortex_review.py share this

mkdir -p "$STATE_DIR"
log() { echo "[$(date -Is)] $*" | tee -a "$LOG"; }

# --- freshness gate: zero cost unless the chat index changed since last run ---
if [[ "$FORCE" != "--force" ]]; then
  if [[ -f "$MARKER" && -d "$INDEX_PATH" ]]; then
    if [[ -z "$(find "$INDEX_PATH" -type f -newer "$MARKER" -print -quit 2>/dev/null)" ]]; then
      log "SKIP: chat index not newer than last run. No new data → no cost."
      exit 0
    fi
  fi
fi

SINCE="$(date -Is -r "$MARKER" 2>/dev/null | cut -dT -f1 || true)"
[[ -z "$SINCE" ]] && SINCE="$(date -d '30 days ago' +%F 2>/dev/null || date +%F)"
log "PRODUCE: filling review queue from chat since $SINCE"

# Producer never writes to the ledger — it queues review items (--review).
set +e
python3 "$SCRIPT_DIR/consolidate_run.py" "recent since $SINCE" --review 2>&1 | tee -a "$LOG"
rc=${PIPESTATUS[0]}
set -e

if [[ $rc -ne 0 ]]; then
  log "ERROR: producer exited $rc — marker unchanged so next run retries."
  exit $rc
fi

touch "$MARKER"
pending=$(ls "$STATE_DIR/review/pending"/*.json 2>/dev/null | wc -l | tr -d ' ')
log "OK: marker advanced. review queue pending=$pending (drain: cortex_review.py list)"

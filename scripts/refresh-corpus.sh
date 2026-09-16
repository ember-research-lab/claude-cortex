#!/usr/bin/env bash
# refresh-corpus.sh — the deterministic tail of chat-export automation.
#
# newest claude.ai export zip in ~/Downloads  ->  ember-chat-search index (+ semantic)
#   ->  trigger the consolidation producer (fills the review queue).
#
# Idempotent (per-zip marker). Event-driven by cortex-corpus-refresh.path (watches
# ~/Downloads), or manual: bash refresh-corpus.sh [/path/to/export.zip]
#
# This is the half that works WITHOUT browser auth: once a zip lands (you click the
# "Download Data" email link, or the Playwright layer fetches it), everything
# downstream is hands-off. Requesting+downloading the export is the auth-gated half.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ECS="${EMBER_CHAT_SEARCH:-$HOME/projects/ember-chat-search/target/release/ember-chat-search}"
DOWNLOADS="${CORTEX_EXPORT_DIR:-$HOME/Downloads}"
STATE_DIR="${CORTEX_CONSOLIDATE_STATE:-$HOME/.local/state/cortex-consolidate}"
MARKER="$STATE_DIR/last-ingested-zip"
LOG="$STATE_DIR/consolidate.log"
mkdir -p "$STATE_DIR"
log() { echo "[$(date -Is)] refresh-corpus: $*" | tee -a "$LOG"; }

# pick the zip: explicit arg, else newest claude.ai export in Downloads
ZIP="${1:-}"
[[ -z "$ZIP" ]] && ZIP="$(ls -t "$DOWNLOADS"/data-*.zip 2>/dev/null | head -1 || true)"
if [[ -z "$ZIP" || ! -f "$ZIP" ]]; then
  log "no export zip found in $DOWNLOADS — nothing to ingest."
  exit 0
fi

sig="$ZIP:$(stat -c %Y "$ZIP" 2>/dev/null || echo 0)"
if [[ -f "$MARKER" && "$(cat "$MARKER" 2>/dev/null)" == "$sig" ]]; then
  log "already ingested $ZIP — skip."
  exit 0
fi

if [[ ! -x "$ECS" ]]; then
  log "ERROR: ember-chat-search binary not found at $ECS"; exit 1
fi

log "indexing $ZIP"
"$ECS" index "$ZIP" 2>&1 | tee -a "$LOG"
"$ECS" semantic-index 2>&1 | tee -a "$LOG" || log "semantic-index failed (non-fatal)"
echo "$sig" > "$MARKER"
log "indexed OK — triggering consolidation producer"
bash "$SCRIPT_DIR/chat-consolidate.sh" --force 2>&1 | tee -a "$LOG" || log "consolidate producer returned nonzero"
log "done — drain with: cortex_review.py list"

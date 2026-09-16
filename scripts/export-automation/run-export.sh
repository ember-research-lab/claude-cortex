#!/usr/bin/env bash
# run-export.sh — scheduled entrypoints for the claude.ai export loop.
#   run-export.sh request    → ask claude.ai for a fresh export (headed Playwright; no email needed)
#   run-export.sh retrieve   → Gmail-IMAP check for a ready export → download → (refresh-corpus.path
#                              then indexes + consolidates automatically)
#
# The two run on SEPARATE schedules because the export is async (email arrives minutes–hours
# after the request). Detects an expired session cookie (auth failure) and alerts to re-import.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_DIR="${CORTEX_CONSOLIDATE_STATE:-$HOME/.local/state/cortex-consolidate}"
LOG="$STATE_DIR/export.log"
mkdir -p "$STATE_DIR"
# shellcheck disable=SC1091
[ -f "$HOME/.cortex-consolidate.env" ] && source "$HOME/.cortex-consolidate.env"
NTFY="${CORTEX_CONSOLIDATE_NTFY:-}"
log() { echo "[$(date -Is)] run-export: $*" | tee -a "$LOG"; }
alert() { [ -n "$NTFY" ] && curl -fsS -d "$1" "https://ntfy.sh/$NTFY" >/dev/null 2>&1 || true; }

case "${1:-retrieve}" in
  request)
    log "requesting fresh export"
    if node "$DIR/export-bot.js" request >>"$LOG" 2>&1; then
      log "export requested OK (email arrives when ready; retrieve will catch it)"
    else
      log "request FAILED — session cookie likely expired; re-import: node export-bot.js import-cookie"
      alert "cortex export: request FAILED — re-import claude.ai sessionKey"
      exit 1
    fi
    ;;
  retrieve)
    url="$(python3 "$DIR/retrieve.py" 2>>"$LOG" || true)"
    if [ -z "$url" ]; then log "no new export ready"; exit 0; fi
    log "export ready — downloading"
    if node "$DIR/export-bot.js" download "$url" >>"$LOG" 2>&1; then
      log "downloaded OK — cortex-corpus-refresh.path will index + consolidate"
      alert "cortex export: new corpus downloaded + queued for consolidation"
    else
      log "download FAILED — session cookie likely expired; re-import needed"
      alert "cortex export: download FAILED — re-import claude.ai sessionKey"
      exit 1
    fi
    ;;
  *)
    echo "usage: run-export.sh request|retrieve" >&2; exit 2 ;;
esac

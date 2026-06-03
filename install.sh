#!/usr/bin/env bash
#
# claude-cortex binary installer.
#
# Claude Code's plugin loader installs the markdown (agents, skills, hooks.json)
# and plugin.json, but NOT the Rust binaries the hooks and MCP server invoke.
# This script builds them in release mode and installs them onto PATH so that
# `cortex-session-start`, `cortex-post-tool-use`, `cortex-session-end`,
# `cortex-mcp`, `cortex-migrate`, and `cortex-dream` resolve.
#
# Usage:
#   bash install.sh                 # build + install to ~/.local/bin
#   CORTEX_BIN_DIR=~/bin bash install.sh   # install elsewhere on PATH
#
# Re-running is safe (idempotent): it rebuilds and overwrites in place.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="${CORTEX_BIN_DIR:-$HOME/.local/bin}"

BINS="cortex-session-start cortex-post-tool-use cortex-session-end cortex-pre-compact cortex-mcp cortex-migrate cortex-dream"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo not found. Install Rust (>= 1.85) from https://rustup.rs and retry." >&2
  exit 1
fi

echo "==> Building cortex binaries (release)..."
cargo build --release --manifest-path "$ROOT/Cargo.toml" \
  -p cortex-hooks -p cortex-mcp -p cortex-migrate -p cortex-dream

echo "==> Installing to $DEST"
mkdir -p "$DEST"
for b in $BINS; do
  src="$ROOT/target/release/$b"
  if [ ! -x "$src" ]; then
    echo "error: expected build output missing: $src" >&2
    exit 1
  fi
  install -m 0755 "$src" "$DEST/$b"
  echo "    $b"
done

case ":$PATH:" in
  *":$DEST:"*) ;;
  *)
    echo
    echo "WARNING: $DEST is not on your PATH."
    echo "  Add this to your shell profile (~/.zshrc or ~/.bashrc):"
    echo "    export PATH=\"$DEST:\$PATH\""
    ;;
esac

echo
echo "==> Verifying"
"$DEST/cortex-mcp" --version 2>/dev/null || echo "    (cortex-mcp --version unavailable; binary still installed)"

echo
echo "Done. Restart Claude Code so existing sessions pick up the binaries."

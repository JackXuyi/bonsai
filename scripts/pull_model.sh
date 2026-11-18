#!/usr/bin/env bash
# Pull the default model into the project-local model store (.llm/models).
#   pnpm llm:pull            # qwen3:4b (default)
#   pnpm llm:pull <model>    # any other tag
#
# Uses the sidecar binary itself as CLI/server — no system Ollama required.
# Starts a temporary server on the app's port, pulls, then stops it.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODEL="${1:-qwen3:4b}"
PORT="${BONSAI_LLM_PORT:-11435}"

ARCH="$(uname -m)"
case "$ARCH" in
  arm64) TARGET="aarch64-apple-darwin" ;;
  x86_64) TARGET="x86_64-apple-darwin" ;;
  *) echo "unsupported arch: $ARCH" >&2; exit 1 ;;
esac

BIN="$ROOT/src-tauri/binaries/ollama-$TARGET"
[ -x "$BIN" ] || { echo "sidecar missing — run 'pnpm llm:sidecar' first" >&2; exit 1; }

MODELS_DIR="$ROOT/.llm/models"
mkdir -p "$MODELS_DIR"

export OLLAMA_MODELS="$MODELS_DIR"
export OLLAMA_HOST="127.0.0.1:$PORT"

"$BIN" serve &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null' EXIT

# wait for the server
up=0
for i in $(seq 1 30); do
  if curl -sf "http://127.0.0.1:$PORT/api/version" >/dev/null; then
    up=1
    break
  fi
  sleep 1
done
[ "$up" -eq 1 ] || { echo "error: temporary ollama server failed to start (see output above)" >&2; exit 1; }

echo "pulling $MODEL into $MODELS_DIR ..."
"$BIN" pull "$MODEL"
echo "done."

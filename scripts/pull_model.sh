#!/usr/bin/env bash
# Pull the default model into the project-local model store (.llm/models).
#   pnpm llm:pull            # qwen3.5:2b (default)
#   pnpm llm:pull <model>    # any other tag
#
# Uses the sidecar binary itself as CLI/server — no system Ollama required.
# Starts a temporary server on the app's port, pulls, then stops it.
#
# China-friendly fallback: if `ollama pull` fails (registry.ollama.ai has no
# official mainland-China mirror), known models are re-fetched as official GGUF
# from ModelScope (Aliyun domestic CDN) or a locally available GGUF copy, and
# imported via `ollama create`.
# Env overrides: BONSAI_GGUF_URL, BONSAI_LOCAL_GGUF, BONSAI_LLM_PORT.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODEL="${1:-qwen3.5:2b}"
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

# Already in the store? Nothing to do.
if "$BIN" show "$MODEL" >/dev/null 2>&1; then
  echo "model already present: $MODEL"
  exit 0
fi

# Fast path — reuse a GGUF already sitting on this machine. The default
# location is the model cache of us.followup.dev (another local app that
# ships the same Qwen3.5-2B Q4_K_M GGUF); override with BONSAI_LOCAL_GGUF.
LOCAL_GGUF="${BONSAI_LOCAL_GGUF:-}"
case "$MODEL" in
  qwen3.5:2b)
    LOCAL_GGUF="${LOCAL_GGUF:-$HOME/Library/Application Support/us.followup.dev/models/summary/Qwen3.5-2B-Q4_K_M.gguf}"
    ;;
esac

GGUF_DIR="$MODELS_DIR/gguf"
mkdir -p "$GGUF_DIR"

if [ -n "$LOCAL_GGUF" ] && [ -f "$LOCAL_GGUF" ]; then
  echo "importing $MODEL from local GGUF: $LOCAL_GGUF"
  GGUF="$GGUF_DIR/$(basename "$LOCAL_GGUF")"
  if [ ! -f "$GGUF" ]; then
    cp "$LOCAL_GGUF" "$GGUF.part" && mv "$GGUF.part" "$GGUF"
  fi
  printf 'FROM %s\n' "$GGUF" > "$GGUF_DIR/Modelfile"
  "$BIN" create "$MODEL" -f "$GGUF_DIR/Modelfile"
  echo "done (imported from local GGUF)."
  exit 0
fi

echo "pulling $MODEL into $MODELS_DIR ..."
if "$BIN" pull "$MODEL"; then
  echo "done."
  exit 0
fi

# registry.ollama.ai has no official mainland-China mirror and is frequently
# unreachable or throttled. Fallback: download the official Qwen GGUF from
# ModelScope (Aliyun domestic CDN) and import it with `ollama create`.
GGUF_URL="${BONSAI_GGUF_URL:-}"
case "$MODEL" in
  qwen3:4b)
    # Legacy default; ModelScope still mirrors its GGUF.
    GGUF_URL="${GGUF_URL:-https://modelscope.cn/models/Qwen/Qwen3-4B-GGUF/resolve/master/Qwen3-4B-Q4_K_M.gguf}"
    GGUF_NAME="Qwen3-4B-Q4_K_M.gguf"
    ;;
  *)
    # No known China GGUF mirror for other tags (e.g. qwen3.5:2b) — the
    # registry path above is the only source. Set BONSAI_GGUF_URL to inject one.
    ;;
esac

if [ -z "$GGUF_URL" ]; then
  echo "error: pull failed and no China-mirror GGUF fallback is configured for '$MODEL'" >&2
  echo "hint: retry with a reachable network, or set BONSAI_GGUF_URL=<gguf url>" >&2
  exit 1
fi

GGUF_NAME="${GGUF_NAME:-$(basename "${GGUF_URL%%\?*}")}"

echo "pull failed (registry.ollama.ai unreachable?) — falling back to downloaded GGUF"
GGUF="$GGUF_DIR/$GGUF_NAME"

if [ ! -f "$GGUF" ]; then
  echo "downloading $GGUF_URL ..."
  if curl -fL --connect-timeout 15 --retry 3 --progress-bar "$GGUF_URL" -o "$GGUF.part" \
     && [ -s "$GGUF.part" ]; then
    mv "$GGUF.part" "$GGUF"
  else
    rm -f "$GGUF.part"
    echo "error: GGUF download failed from $GGUF_URL" >&2
    exit 1
  fi
else
  echo "GGUF already cached: $GGUF"
fi

printf 'FROM %s\n' "$GGUF" > "$GGUF_DIR/Modelfile"
"$BIN" create "$MODEL" -f "$GGUF_DIR/Modelfile"
echo "done (imported from ModelScope GGUF)."

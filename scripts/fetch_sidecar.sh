#!/usr/bin/env bash
# Fetch the Ollama sidecar binary for the current platform and place it where
# Tauri's externalBin expects it (src-tauri/binaries/ollama-<target-triple>).
#
# This is the ONLY manual step of the "sidecar bundled with the app" design:
#   pnpm llm:sidecar
#
# The binary is a build asset, not a repo file — src-tauri/binaries/ is
# gitignored. macOS only for now (project platform order: macOS first).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="$ROOT/src-tauri/binaries"
mkdir -p "$BIN_DIR"

ARCH="$(uname -m)"
case "$ARCH" in
  arm64) TARGET="aarch64-apple-darwin"; ARCHIVE="Ollama-darwin.zip" ;;
  x86_64) TARGET="x86_64-apple-darwin"; ARCHIVE="Ollama-darwin.zip" ;;
  *) echo "unsupported arch: $ARCH (macOS only for now)" >&2; exit 1 ;;
esac

DEST="$BIN_DIR/ollama-$TARGET"

# build.rs generates a tiny placeholder script at the same path when the real
# binary is missing (keeps cargo check / tauri dev working). Treat a file that
# small / self-labelled as "not installed" so the real download can proceed.
is_placeholder() {
  [ "$(wc -c < "$DEST" 2>/dev/null || echo 0)" -lt 1048576 ] \
    && head -c 256 "$DEST" 2>/dev/null | grep -q "bonsai placeholder"
}

if [ -x "$DEST" ] && ! is_placeholder; then
  echo "sidecar already present: $DEST"
  "$DEST" --version || true
  exit 0
fi

if is_placeholder; then
  echo "placeholder detected at $DEST — fetching the real binary"
  rm -f "$DEST"
fi

VERSION="${OLLAMA_VERSION:-0.40.16}"
GH_URL="https://github.com/ollama/ollama/releases/download/v${VERSION}/${ARCHIVE}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Download sources, tried in order. GitHub release downloads are frequently
# slow/unstable from mainland China, so a community gh-proxy mirror is tried
# first and direct GitHub is kept as the fallback. Override the whole list
# with BONSAI_GH_PROXY (e.g. your own mirror; set BONSAI_GH_PROXY="" to go
# direct only).
if [ -n "${BONSAI_GH_PROXY+x}" ]; then
  SOURCES=("${BONSAI_GH_PROXY%/}/$GH_URL")
else
  SOURCES=(
    "https://gh-proxy.com/$GH_URL"
    "$GH_URL"
  )
fi

ok=""
for url in "${SOURCES[@]}"; do
  echo "downloading $url ..."
  if curl -fL --connect-timeout 15 --retry 2 --progress-bar "$url" -o "$TMP/ollama.zip" \
     && unzip -tqq "$TMP/ollama.zip" >/dev/null 2>&1; then
    ok=1
    break
  fi
  echo "source failed, trying next ..." >&2
  rm -f "$TMP/ollama.zip"
done
[ -n "$ok" ] || { echo "all download sources failed: $GH_URL" >&2; exit 1; }

echo "extracting ..."
unzip -q -o "$TMP/ollama.zip" -d "$TMP/app"

# The CLI binary lives inside the app bundle.
SRC="$TMP/app/Ollama.app/Contents/Resources/ollama"
[ -f "$SRC" ] || SRC="$(find "$TMP/app" -type f -name ollama -perm +111 | head -1)"
[ -n "${SRC:-}" ] && [ -f "$SRC" ] || { echo "ollama CLI not found in archive" >&2; exit 1; }

install -m 0755 "$SRC" "$DEST"
echo "sidecar installed: $DEST"
"$DEST" --version

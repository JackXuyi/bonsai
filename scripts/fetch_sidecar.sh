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
URL="https://github.com/ollama/ollama/releases/download/v${VERSION}/${ARCHIVE}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "downloading $URL ..."
curl -fL --progress-bar "$URL" -o "$TMP/ollama.zip"

echo "extracting ..."
unzip -q -o "$TMP/ollama.zip" -d "$TMP/app"

# The CLI binary lives inside the app bundle.
SRC="$TMP/app/Ollama.app/Contents/Resources/ollama"
[ -f "$SRC" ] || SRC="$(find "$TMP/app" -type f -name ollama -perm +111 | head -1)"
[ -n "${SRC:-}" ] && [ -f "$SRC" ] || { echo "ollama CLI not found in archive" >&2; exit 1; }

install -m 0755 "$SRC" "$DEST"
echo "sidecar installed: $DEST"
"$DEST" --version

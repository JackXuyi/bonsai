#!/usr/bin/env bash
# Fetch the Ollama sidecar binary for the current platform and place it where
# Tauri's externalBin expects it (src-tauri/binaries/ollama-<target-triple>).
#
# This is the ONLY manual step of the "sidecar bundled with the app" design:
#   pnpm llm:sidecar
#
# The binary is a build asset, not a repo file — src-tauri/binaries/ is
# gitignored. Supported: macOS (arm64/x86_64) and Windows x86_64.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="$ROOT/src-tauri/binaries"
mkdir -p "$BIN_DIR"

case "$(uname -s)" in
  Darwin)
    PLATFORM="macos"
    case "$(uname -m)" in
      arm64) TARGET="aarch64-apple-darwin" ;;
      x86_64) TARGET="x86_64-apple-darwin" ;;
      *) echo "unsupported macOS arch: $(uname -m)" >&2; exit 1 ;;
    esac
    ARCHIVE="Ollama-darwin.zip"
    DEST="$BIN_DIR/ollama-$TARGET"
    RUNNER="llama-server"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    PLATFORM="windows"
    TARGET="x86_64-pc-windows-msvc"
    ARCHIVE="ollama-windows-amd64.zip"
    DEST="$BIN_DIR/ollama-$TARGET.exe"
    RUNNER="llama-server.exe"
    ;;
  *)
    echo "unsupported platform: $(uname -s) (macOS and Windows x64 only)" >&2
    exit 1
    ;;
esac

# Since v0.40 the inference engine is a separate runner executable plus its
# shared libraries, not embedded in the `ollama` binary. Both live flat next
# to each other; we stage them in `ollama-runtime/` and either symlink
# (macOS) or copy (Windows) them beside the CLI so the CLI finds the runner.
RUNTIME_DIR="$BIN_DIR/ollama-runtime"

# build.rs generates a tiny placeholder script at the same path when the real
# binary is missing (keeps cargo check / tauri dev working). Treat a file that
# small / self-labelled as "not installed" so the real download can proceed.
is_placeholder() {
  [ "$(wc -c < "$DEST" 2>/dev/null || echo 0)" -lt 1048576 ] \
    && head -c 256 "$DEST" 2>/dev/null | grep -q "bonsai placeholder"
}

have_runtime() { [ -x "$RUNTIME_DIR/$RUNNER" ] || [ -f "$RUNTIME_DIR/$RUNNER" ]; }

if [ -x "$DEST" ] && ! is_placeholder && have_runtime; then
  echo "sidecar already present: $DEST"
  echo "runtime already present: $RUNTIME_DIR"
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

# GitHub release downloads are frequently slow/unstable from mainland China,
# so a community gh-proxy mirror is tried first on macOS, with direct GitHub
# as fallback. Windows CI downloads direct (same-region, much faster).
# Override with BONSAI_GH_PROXY (e.g. your own mirror; set to "" for direct).
if [ -n "${BONSAI_GH_PROXY+x}" ]; then
  SOURCES=("${BONSAI_GH_PROXY%/}/$GH_URL")
elif [ "$PLATFORM" = "windows" ]; then
  SOURCES=("$GH_URL")
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

rm -rf "$RUNTIME_DIR"
mkdir -p "$RUNTIME_DIR"

if [ "$PLATFORM" = "macos" ]; then
  RES="$(find "$TMP/app" -type d -name Resources -maxdepth 4 | head -1)"
  [ -n "${RES:-}" ] || { echo "Ollama.app/Contents/Resources not found in archive" >&2; exit 1; }

  SRC="$RES/ollama"
  [ -f "$SRC" ] || { echo "ollama CLI not found in archive" >&2; exit 1; }
  install -m 0755 "$SRC" "$DEST"
  echo "sidecar installed: $DEST"

  # Runner + shared libraries. MLX runtime dirs are copied when present, but
  # optional — GGUF imports via `ollama create` do not need them.
  echo "staging runtime into $RUNTIME_DIR ..."
  install -m 0755 "$RES/llama-server" "$RUNTIME_DIR/llama-server"
  for f in "$RES"/libllama*.dylib "$RES"/libggml*.dylib "$RES"/libmtmd*.dylib; do
    [ -e "$f" ] && cp -a "$f" "$RUNTIME_DIR/"
  done
  for mlx in "$RES"/mlx_metal_v3 "$RES"/mlx_metal_v4; do
    [ -d "$mlx" ] && cp -a "$mlx" "$RUNTIME_DIR/"
  done
  echo "runtime installed: $(ls "$RUNTIME_DIR" | wc -l | tr -d ' ') entries, $(du -sh "$RUNTIME_DIR" | cut -f1)"

  # The CLI looks for the runner next to itself. tauri-build / build.rs
  # arrange that inside the app and target/debug; scripts that run this
  # binary directly (pull_model.sh) need relative symlinks here too.
  for entry in "$RUNTIME_DIR"/*; do
    ln -sfn "ollama-runtime/$(basename "$entry")" "$BIN_DIR/$(basename "$entry")"
  done
else
  # Windows portable zip: ollama.exe, llama-server.exe and lib*.dll are flat
  # (possibly under a subfolder). Pick the first match and keep the CPU
  # runtime only — CUDA/ROCm variants are not shipped, same as macOS.
  CLI="$(find "$TMP/app" -type f -name 'ollama.exe' | head -1)"
  RUNNER_BIN="$(find "$TMP/app" -type f -name 'llama-server.exe' | head -1)"
  [ -n "$CLI" ] || { echo "ollama.exe not found in archive" >&2; exit 1; }
  [ -n "$RUNNER_BIN" ] || { echo "llama-server.exe not found in archive" >&2; exit 1; }

  install -m 0755 "$CLI" "$DEST"
  echo "sidecar installed: $DEST"

  install -m 0755 "$RUNNER_BIN" "$RUNTIME_DIR/llama-server.exe"
  while IFS= read -r dll; do
    cp -a "$dll" "$RUNTIME_DIR/"
  done < <(find "$TMP/app" -type f \( -name 'libllama*.dll' -o -name 'libggml*.dll' -o -name 'libmtmd*.dll' \))
  echo "runtime installed: $(ls "$RUNTIME_DIR" | wc -l | tr -d ' ') entries, $(du -sh "$RUNTIME_DIR" | cut -f1)"

  # Windows symlinks need developer mode; copy instead so direct scripts
  # (pull_model.sh) and dev builds find the runner beside the CLI too.
  cp -a "$RUNTIME_DIR/." "$BIN_DIR/"
fi

"$DEST" --version

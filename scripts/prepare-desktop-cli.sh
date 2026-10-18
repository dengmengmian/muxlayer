#!/usr/bin/env bash
# Build the headless CLI for the target and place it where Tauri expects the
# externalBin sidecar. The resulting binary is not committed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-$(rustc -vV | sed -n 's/^host: //p')}"

case "$TARGET" in
  *windows*) EXT=".exe" ;;
  *) EXT="" ;;
esac

BIN="$ROOT/src-tauri/target/$TARGET/release/agentgate-serve$EXT"
DEST="$ROOT/src-tauri/binaries/agentgate-serve-$TARGET$EXT"

cargo build \
  --manifest-path "$ROOT/src-tauri/Cargo.toml" \
  --no-default-features \
  --features cli \
  --bin agentgate-serve \
  --target "$TARGET" \
  --release

if [ ! -x "$BIN" ]; then
  echo "expected built CLI at $BIN" >&2
  exit 1
fi

mkdir -p "$(dirname "$DEST")"
cp "$BIN" "$DEST"
chmod +x "$DEST"
echo "prepared $DEST"

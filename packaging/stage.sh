#!/usr/bin/env bash
# Assemble the self-contained app folder `dist/Agentory/` (what the installer / tarball ships).
#
#   packaging/stage.sh
#
# Layout:
#   agentory(.exe)      the app (fonts and icons are embedded)
#   agentory-helper.exe Windows only: launches the app and swaps/reverts self-built exes
#   LICENSE, NOTICE.txt
#
# Needs: cargo. `claude` and `git` are looked up on the user's PATH at run time.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$HERE/dist/Agentory"

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) EXE=.exe ;;
  Linux)                EXE= ;;
  *) echo "unsupported platform $(uname -s)" >&2; exit 1 ;;
esac

echo "==> cargo build --release"
(cd "$HERE" && cargo build --release --locked)
[ -n "$EXE" ] && (cd "$HERE" && cargo build --release --locked -p agentory-helper)

rm -rf "$OUT"
mkdir -p "$OUT"
cp "$HERE/target/release/agentory$EXE" "$OUT/"
[ -n "$EXE" ] && cp "$HERE/target/release/agentory-helper$EXE" "$OUT/" || true

cp "$HERE/packaging/NOTICE.txt" "$OUT/NOTICE.txt"
[ -f "$HERE/LICENSE" ] && cp "$HERE/LICENSE" "$OUT/LICENSE" || true
echo "==> staged at $OUT"

#!/usr/bin/env bash
#
# Install a `codez` command-line launcher so you can run `codez` or
# `codez <path>` from any terminal to open the installed CodeZ.app.
#
#   ./scripts/install-cli.sh            # installs to /usr/local/bin
#   ./scripts/install-cli.sh ~/bin      # installs to a custom dir on PATH
#
# Requires CodeZ.app to already be installed (in /Applications or ~/Applications).
#
set -euo pipefail

PREFIX="${1:-/usr/local/bin}"
TARGET="$PREFIX/codez"

# The launcher: find the app, resolve the argument to an absolute path (so it
# works from any cwd), then start the GUI in the background.
read -r -d '' SHIM <<'SH' || true
#!/bin/sh
APP="/Applications/CodeZ.app/Contents/MacOS/CodeZ"
[ -x "$APP" ] || APP="$HOME/Applications/CodeZ.app/Contents/MacOS/CodeZ"
if [ ! -x "$APP" ]; then
  echo "CodeZ.app not found in /Applications or ~/Applications." >&2
  echo "Install it first (open the .dmg and drag CodeZ to Applications)." >&2
  exit 1
fi
target="${1:-$PWD}"
case "$target" in
  /*) ;;
  *) target="$PWD/$target" ;;
esac
"$APP" "$target" >/dev/null 2>&1 &
SH

echo "==> Installing codez launcher to $TARGET"
TMP="$(mktemp)"
printf '%s\n' "$SHIM" > "$TMP"
chmod +x "$TMP"

mkdir -p "$PREFIX" 2>/dev/null || true
if [ -w "$PREFIX" ]; then
  mv "$TMP" "$TARGET"
else
  echo "    $PREFIX is not writable — using sudo"
  sudo mv "$TMP" "$TARGET"
  sudo chmod +x "$TARGET"
fi

echo "Done. Try:  codez .        (open current folder)"
echo "            codez ~/proj   (open a folder)"
case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *) echo "Note: $PREFIX is not on your PATH — add it to use 'codez' directly." ;;
esac

#!/usr/bin/env bash
# Regenerates the browser fixtures: a plain HTML form (form.html) submitted
# natively by headless Chrome and Firefox. Requires google-chrome (or chromium)
# and firefox.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT="$HERE/.."
PORT=18928
CHROME=$(command -v google-chrome || command -v chromium || command -v chromium-browser)

# A snap-packaged Firefox can only read profiles below ~/snap/firefox.
FF_TMP=/tmp
[ -d "$HOME/snap/firefox/common" ] && FF_TMP="$HOME/snap/firefox/common"

capture() { # name, profile parent dir, command...
  local name=$1 parent=$2; shift 2
  local prof
  prof=$(mktemp -d -p "$parent" nostd-multipart-capture.XXXXXX)
  python3 "$HERE/server.py" "$OUT" "$PORT" 1 &
  local srv=$!
  sleep 0.5
  "$@" "$prof" "http://127.0.0.1:$PORT/?name=$name" >/dev/null 2>&1 &
  local br=$!
  wait $srv
  # The browser runs inside a function subshell: kill it by its profile path.
  pkill -f -- "$prof" 2>/dev/null || true
  wait $br 2>/dev/null || true
  rm -rf "$prof"
}

chrome() { "$CHROME" --headless=new --disable-gpu --no-first-run --user-data-dir="$1" "$2"; }
firefox_() { firefox --headless --no-remote --profile "$1" "$2"; }

capture chrome /tmp chrome
capture firefox "$FF_TMP" firefox_
ls -la "$OUT"/chrome.* "$OUT"/firefox.*

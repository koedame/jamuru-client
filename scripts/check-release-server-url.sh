#!/usr/bin/env bash
# Checks a built release against REQ-DIST-005 (ADR-030): the app must use the
# jamuru server it was built with, and nothing in what ships may point at a
# server on the user's own machine.
#
# Usage (from the repository root, after `cargo tauri build`, with the same
# JAMJAM_SERVER_URL the build was given):
#   scripts/check-release-server-url.sh <app binary> [ui dist directory]
#
#   scripts/check-release-server-url.sh src-tauri/target/release/jamuru-app
#   scripts/check-release-server-url.sh src-tauri/target/x86_64-pc-windows-msvc/release/jamuru-app.exe
#
# The binary carries the Rust side (connection screen and diagnostics); the UI
# bundle is embedded in it compressed, so ui/dist is scanned separately.
# The server's address is not printed: the log of a public workflow is public.
set -euo pipefail

binary=${1:?usage: $0 <app binary> [ui dist directory]}
dist=${2:-ui/dist}

production=${JAMJAM_SERVER_URL:-}
if [ -z "$production" ]; then
  echo "JAMJAM_SERVER_URL is not set - pass the server the release was built with" >&2
  exit 1
fi
for path in "$binary" "$dist"; do
  if [ ! -e "$path" ]; then
    echo "$path does not exist - build the release first" >&2
    exit 1
  fi
done

loopback='(https?|wss?)://(localhost|127\.[0-9.]+|\[::1\]|\[0:0:0:0:0:0:0:1\]|0\.0\.0\.0)'
failed=0

# Without the production URL in the binary, this is not the release build
# (a debug build dials localhost) and the scan below would prove nothing.
if ! grep -aqF "$production" "$binary"; then
  echo "FAIL: $binary does not contain JAMJAM_SERVER_URL" >&2
  failed=1
fi

# Tauri keeps the development UI's address (devUrl in tauri.conf.json) in every build. A release
# loads the bundled UI instead, so that one local URL is not a server the app talks to.
dev_ui=$(sed -n 's/.*"devUrl": *"\([^"]*\)".*/\1/p' src-tauri/tauri.conf.json)
if found=$(grep -aoiE "$loopback(:[0-9]+)?" "$binary" | sort -u | grep -vixF "$dev_ui") && [ -n "$found" ]; then
  echo "FAIL: $binary names a local server:" >&2
  echo "$found" >&2
  failed=1
fi

if found=$(grep -rliE "$loopback" "$dist") && [ -n "$found" ]; then
  echo "FAIL: the UI bundle names a local server:" >&2
  echo "$found" >&2
  failed=1
fi

if [ "$failed" -ne 0 ]; then
  exit 1
fi
echo "OK: $binary uses the server it was built with and names no local server"

#!/usr/bin/env bash
# Checks that a built app carries remote debugging only when it should (ADR-044).
#
# A beta build is made with the `debug-remote` feature and a release build
# without it. The feature leaves a marker in the binary; this looks for it, so a
# release that has the code, or a beta that lost it, is stopped before anything
# is published.
#
# Usage:
#   scripts/check-debug-remote.sh <app binary> present   # a beta build
#   scripts/check-debug-remote.sh <app binary> absent    # a release build
set -euo pipefail

binary=${1:?usage: $0 <app binary> <present|absent>}
expected=${2:?usage: $0 <app binary> <present|absent>}
marker='jamjam-debug-remote/1'

case "$expected" in
  present | absent) ;;
  *)
    echo "expected must be 'present' or 'absent', got '$expected'" >&2
    exit 2
    ;;
esac
if [ ! -f "$binary" ]; then
  echo "$binary does not exist - build the app first" >&2
  exit 1
fi

# The marker says the connection code is in. The method names say the tools it
# would serve are: a release must have none of them, a beta must have the marker.
tools=('debug.info' 'ui.query')

found=absent
if grep -aqF "$marker" "$binary"; then
  found=present
fi
if [ "$expected" = absent ]; then
  for name in "${tools[@]}"; do
    if grep -aqF "$name" "$binary"; then
      echo "FAIL: $binary contains the debug method $name, which a release build must not" >&2
      exit 1
    fi
  done
fi

if [ "$found" != "$expected" ]; then
  echo "FAIL: remote debugging is $found in $binary, but this build must have it $expected" >&2
  exit 1
fi
echo "ok: remote debugging is $found, as expected"

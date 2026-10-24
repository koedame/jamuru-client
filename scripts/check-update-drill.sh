#!/usr/bin/env bash
# Checks that a built app can be broken on purpose by the update drill only
# when it is one of the drill's builds (ADR-089).
#
# The `update-drill` feature lets an environment variable make the app fail to
# start. It leaves the variable's name in the binary; this looks for it, so a
# release that has the code, or a drill build that lost it, is stopped.
#
# Usage:
#   scripts/check-update-drill.sh <app binary> present   # a drill build
#   scripts/check-update-drill.sh <app binary> absent    # a release or beta build
set -euo pipefail

binary=${1:?usage: $0 <app binary> <present|absent>}
expected=${2:?usage: $0 <app binary> <present|absent>}
marker='JAMJAM_UPDATE_DRILL_BREAK'

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

found=absent
if grep -aqF "$marker" "$binary"; then
  found=present
fi
if [ "$found" != "$expected" ]; then
  echo "FAIL: the update drill's breaking is $found in $binary, but this build must have it $expected" >&2
  exit 1
fi
echo "ok: the update drill's breaking is $found, as expected"

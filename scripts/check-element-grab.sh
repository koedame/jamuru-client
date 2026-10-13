#!/usr/bin/env bash
# Checks that a built screen (ui/dist) carries the Alt+Shift element picker only
# when it should (REQ-RMT-033).
#
# A beta build is made with VITE_ELEMENT_GRAB=true and a release build without
# it. The picker's overlay id is a string the bundler cannot drop from a build
# that has it; this looks for it, so a release that has the picker, or a beta
# that lost it, is stopped before anything is published.
#
# Usage:
#   scripts/check-element-grab.sh <ui/dist> present   # a beta build
#   scripts/check-element-grab.sh <ui/dist> absent    # a release build
set -euo pipefail

dist=${1:?usage: $0 <ui/dist> <present|absent>}
expected=${2:?usage: $0 <ui/dist> <present|absent>}
marker='jamuru-element-grab-overlay'

case "$expected" in
  present | absent) ;;
  *)
    echo "expected must be 'present' or 'absent', got '$expected'" >&2
    exit 2
    ;;
esac
if [ ! -d "$dist" ]; then
  echo "$dist does not exist - build the screen first" >&2
  exit 1
fi

found=absent
if grep -rqF "$marker" "$dist"; then
  found=present
fi
if [ "$found" != "$expected" ]; then
  echo "FAIL: the element picker is $found in $dist, but this build must have it $expected" >&2
  exit 1
fi
echo "ok: the element picker is $found, as expected"

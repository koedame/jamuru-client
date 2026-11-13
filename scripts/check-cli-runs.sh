#!/usr/bin/env bash
# Starts a `jamuru` command-line tool and checks it answers `--version` and
# `--help`: the binary is for the right architecture, finds the libraries it
# links (ALSA) and its loader, and reaches `main`. It says nothing about audio
# on the real board.
#
# Usage:
#   scripts/check-cli-runs.sh <binary> [<sysroot, e.g. /usr/arm-linux-gnueabihf>]
#
# With a sysroot, the binary is a cross build and runs under qemu-user
# (`qemu-arm`), which needs the target's libraries under <sysroot> (the armhf
# ALSA package puts them in <sysroot>/../lib/<triple>, which QEMU_LD_PREFIX makes
# qemu search). Without one, the binary was built on this machine's
# architecture and runs as it is.
set -euo pipefail

binary=${1:?usage: $0 <binary> [<sysroot>]}
sysroot=${2:-}

if [ ! -f "$binary" ]; then
  echo "$binary does not exist - build it first" >&2
  exit 1
fi

run=()
if [ -n "$sysroot" ]; then
  export QEMU_LD_PREFIX="$sysroot"
  export QEMU_SET_ENV="LD_LIBRARY_PATH=/usr/lib/arm-linux-gnueabihf"
  run=(qemu-arm)
fi

version=$("${run[@]}" "$binary" --version)
echo "$version"
case "$version" in
  jamuru\ *) ;;
  *) echo "FAIL: --version printed '$version'" >&2; exit 1 ;;
esac

help=$("${run[@]}" "$binary" --help)
if ! printf '%s\n' "$help" | grep -q 'join-room'; then
  echo "FAIL: --help does not list the join-room command" >&2
  printf '%s\n' "$help" >&2
  exit 1
fi
echo "OK: $binary starts${run:+ under ${run[0]}}"

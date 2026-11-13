#!/usr/bin/env bash
# Checks that a Linux build does not need a newer glibc than the one we promise.
#
# A binary built on a newer Ubuntu asks for that Ubuntu's glibc, and then
# refuses to start anywhere older ("version `GLIBC_2.43' not found"). The build
# runs on a fixed old Ubuntu so it stays within the promised floor; this stops a
# release when a runner change quietly moves the floor up.
#
# Usage:
#   scripts/check-glibc-floor.sh <app binary> <highest allowed glibc, e.g. 2.35>
#
# `OBJDUMP` names the objdump that reads the binary: a cross build needs the
# one for its architecture (`arm-linux-gnueabihf-objdump`).
set -euo pipefail

binary=${1:?usage: $0 <app binary> <highest allowed glibc>}
allowed=${2:?usage: $0 <app binary> <highest allowed glibc>}

if [ ! -f "$binary" ]; then
  echo "$binary does not exist - build the app first" >&2
  exit 1
fi

needed=$("${OBJDUMP:-objdump}" -T "$binary" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n 1)
if [ -z "$needed" ]; then
  echo "$binary asks for no glibc version - is it a Linux binary?" >&2
  exit 1
fi

highest=$(printf '%s\n%s\n' "$needed" "$allowed" | sort -V | tail -n 1)
if [ "$highest" != "$allowed" ]; then
  echo "$binary needs glibc $needed, more than the $allowed we promise" >&2
  exit 1
fi
echo "$binary needs glibc $needed (allowed up to $allowed)"

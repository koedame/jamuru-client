#!/usr/bin/env bash
# An `ldd` that can read 32-bit ARM programs on an amd64 machine.
#
# linuxdeploy, which Tauri's bundler runs to copy an AppImage's libraries into
# it, asks `ldd` what a program needs. The amd64 `ldd` only knows the amd64
# loaders, answers "not a dynamic executable" for an armhf program, and
# linuxdeploy then copies none of its libraries ("is not linked dynamically"):
# the AppImage is built, and lacks WebKitGTK and GTK. The armhf loader can list
# a program's libraries itself (`ld.so --list`); qemu-user runs it.
#
# Put a link to this file called `ldd` first in PATH. An argument that is not
# an ARM ELF file goes to the real ldd.
#
# Usage:
#   scripts/armhf-ldd.sh <program>
set -euo pipefail

loader=/lib/ld-linux-armhf.so.3

# e_machine (bytes 18-19 of an ELF header) is 0x28 for ARM.
is_arm_elf() {
  [ -f "$1" ] && [ "$(head -c 4 "$1" | od -An -c | tr -d ' ')" = '177ELF' ] \
    && [ "$(od -An -tx1 -j18 -N2 "$1" | tr -d ' ')" = '2800' ]
}

if [ "$#" -eq 1 ] && is_arm_elf "$1"; then
  exec qemu-arm-static "$loader" --list "$1"
fi
exec /usr/bin/ldd "$@"

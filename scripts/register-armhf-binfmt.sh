#!/usr/bin/env bash
# Lets the kernel run 32-bit ARM programs through qemu-user, including the
# AppImage tools Tauri's bundler runs when it cross-builds the armhf AppImage.
#
# `qemu-user-static` registers `qemu-arm` for ELF files whose header bytes 8-15
# (the padding after EI_OSABI) are zero. An AppImage's runtime writes its marker
# there ("AI\x02"), so the kernel does not match it and running it fails with
# "Exec format error" (linuxdeploy-plugin-appimage and linuxdeploy itself are
# such files). This registers the same handler with bytes 7-15 ignored.
#
# Usage (as root or with sudo available), after installing qemu-user-static:
#   scripts/register-armhf-binfmt.sh
set -euo pipefail

sudo=""
if [ "$(id -u)" -ne 0 ]; then sudo="sudo"; fi

register=/proc/sys/fs/binfmt_misc/register
handler=/usr/libexec/qemu-binfmt/arm-binfmt-P
if [ ! -w "$register" ] && [ -z "$sudo" ]; then
  echo "binfmt_misc is not mounted or not writable ($register)" >&2
  exit 1
fi
if [ ! -x "$handler" ]; then
  echo "$handler does not exist - install qemu-user-static first" >&2
  exit 1
fi

# ELF32, little endian, version 1, ET_EXEC or ET_DYN, EM_ARM.
magic='\x7f\x45\x4c\x46\x01\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x28\x00'
mask='\xff\xff\xff\xff\xff\xff\xff\x00\x00\x00\x00\x00\x00\x00\x00\x00\xfe\xff\xff\xff'

if [ -e /proc/sys/fs/binfmt_misc/qemu-arm ]; then
  echo -1 | $sudo tee /proc/sys/fs/binfmt_misc/qemu-arm > /dev/null
fi
echo ":qemu-arm:M:0:${magic}:${mask}:${handler}:PF" | $sudo tee "$register" > /dev/null
cat /proc/sys/fs/binfmt_misc/qemu-arm

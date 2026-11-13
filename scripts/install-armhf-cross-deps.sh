#!/usr/bin/env bash
# Installs what it takes to cross-build jamuru for 32-bit ARM Linux
# (`armv7-unknown-linux-gnueabihf`) on an amd64 Ubuntu 22.04: the cross
# compiler and the armhf ALSA headers and library that `cpal` links. With `gui`
# it also installs the armhf libraries the app's window is built against
# (GTK, WebKitGTK, librsvg, OpenSSL) and patchelf, which the bundlers use.
#
# The armhf packages are not on the amd64 mirrors; they come from
# ports.ubuntu.com. The existing sources are pinned to amd64 first, or apt
# would ask the amd64 mirrors for armhf indexes and fail with 404.
#
# Usage (as root or with sudo available):
#   scripts/install-armhf-cross-deps.sh [gui]
set -euo pipefail

with_gui=false
case "${1:-}" in
  "") ;;
  gui) with_gui=true ;;
  *) echo "usage: $0 [gui]" >&2; exit 2 ;;
esac

sudo=""
if [ "$(id -u)" -ne 0 ]; then sudo="sudo"; fi

# shellcheck source=/dev/null
codename=$(. /etc/os-release && echo "${VERSION_CODENAME}")

# One-line sources (`deb http://... jammy main`) and deb822 sources (`Types: deb`).
for list in /etc/apt/sources.list /etc/apt/sources.list.d/*.list; do
  [ -f "$list" ] || continue
  $sudo sed -i -E '/^deb(-src)? / { /\[arch=/! s/^(deb(-src)?) /\1 [arch=amd64] / }' "$list"
done
for list in /etc/apt/sources.list.d/*.sources; do
  [ -f "$list" ] || continue
  grep -q '^Architectures:' "$list" || $sudo sed -i 's/^Types: deb$/Types: deb\nArchitectures: amd64/' "$list"
done

$sudo dpkg --add-architecture armhf
printf '%s\n' \
  "deb [arch=armhf] http://ports.ubuntu.com/ubuntu-ports ${codename} main universe" \
  "deb [arch=armhf] http://ports.ubuntu.com/ubuntu-ports ${codename}-updates main universe" \
  "deb [arch=armhf] http://ports.ubuntu.com/ubuntu-ports ${codename}-security main universe" \
  | $sudo tee /etc/apt/sources.list.d/armhf-ports.list >/dev/null

$sudo apt-get update
$sudo apt-get install -y --no-install-recommends \
  gcc-arm-linux-gnueabihf \
  libc6-dev-armhf-cross \
  libasound2-dev:armhf \
  pkg-config \
  cmake \
  binutils

if $with_gui; then
  $sudo apt-get install -y --no-install-recommends \
    libwebkit2gtk-4.1-dev:armhf \
    libgtk-3-dev:armhf \
    librsvg2-dev:armhf \
    libssl-dev:armhf \
    patchelf
fi

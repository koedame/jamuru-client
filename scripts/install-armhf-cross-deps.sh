#!/usr/bin/env bash
# Installs what it takes to cross-build the CLI for 32-bit ARM Linux
# (`armv7-unknown-linux-gnueabihf`) on an amd64 Ubuntu 22.04: the cross
# compiler and the armhf ALSA headers and library that `cpal` links.
#
# The armhf packages are not on the amd64 mirrors; they come from
# ports.ubuntu.com. The existing sources are pinned to amd64 first, or apt
# would ask the amd64 mirrors for armhf indexes and fail with 404.
#
# Usage (as root or with sudo available):
#   scripts/install-armhf-cross-deps.sh
set -euo pipefail

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

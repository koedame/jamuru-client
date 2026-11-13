#!/usr/bin/env bash
# Builds the `jamuru` command-line tool for an ARM Linux target and packs it as
# a tar.gz with its checksum.
#
#   armv7-unknown-linux-gnueabihf  32-bit ARM (Debian/Ubuntu "armhf", e.g. a
#                                  NanoPi NEO). Cross-built on an amd64 host;
#                                  needs scripts/install-armhf-cross-deps.sh to
#                                  have run, and the Rust target installed.
#   aarch64-unknown-linux-gnu      64-bit ARM (arm64, e.g. a NanoPi R3S). Built
#                                  natively, on an arm64 host with libasound2-dev,
#                                  pkg-config and cmake installed.
#
# Usage (from the repository root):
#   scripts/build-cli.sh <rust target> <output directory>
#
# Writes <output directory>/jamuru-cli-linux-<armv7|aarch64>.tar.gz and a
# .sha256 next to it. The archive holds `jamuru`, LICENSE and
# THIRD_PARTY_LICENSES.md.
# Built with the default features, the same as the app: no Opus, so no
# libopus is built.
set -euo pipefail

target=${1:?usage: $0 <rust target> <output directory>}
out=${2:?usage: $0 <rust target> <output directory>}

case "$target" in
  armv7-unknown-linux-gnueabihf)
    name=jamuru-cli-linux-armv7
    triple=arm-linux-gnueabihf
    # The oldest glibc the binary may ask for, as with the app (check-glibc-floor.sh).
    glibc_floor=2.35
    cross=1
    ;;
  aarch64-unknown-linux-gnu)
    name=jamuru-cli-linux-aarch64
    triple=aarch64-linux-gnu
    glibc_floor=2.35
    cross=0
    if [ "$(uname -s)-$(uname -m)" != "Linux-aarch64" ]; then
      echo "$target is built natively; this machine is $(uname -s)-$(uname -m)" >&2
      exit 2
    fi
    ;;
  *)
    echo "unsupported target: $target" >&2
    exit 2
    ;;
esac

if [ "$cross" = 1 ]; then
  export PKG_CONFIG_ALLOW_CROSS=1
  export PKG_CONFIG_LIBDIR="/usr/lib/${triple}/pkgconfig:/usr/share/pkgconfig"
  export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER="${triple}-gcc"
  export CC_armv7_unknown_linux_gnueabihf="${triple}-gcc"
  objdump="${triple}-objdump"
  strip="${triple}-strip"
else
  objdump="objdump"
  strip="strip"
fi

cargo build --release --locked --bin jamuru --target "$target"

binary="${CARGO_TARGET_DIR:-target}/${target}/release/jamuru"
OBJDUMP="$objdump" scripts/check-glibc-floor.sh "$binary" "$glibc_floor"

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
mkdir -p "$staging/$name" "$out"
install -m 755 "$binary" "$staging/$name/jamuru"
"$strip" "$staging/$name/jamuru"
cp LICENSE THIRD_PARTY_LICENSES.md "$staging/$name/"
tar -C "$staging" -czf "$out/$name.tar.gz" "$name"
(cd "$out" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "wrote $out/$name.tar.gz"

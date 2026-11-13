#!/usr/bin/env bash
# Cross-builds the `jamuru` command-line tool for a Linux target the desktop app
# does not run on, and packs it as a tar.gz with its checksum.
#
# Only `armv7-unknown-linux-gnueabihf` (32-bit ARM, Debian/Ubuntu "armhf",
# e.g. a NanoPi NEO) is supported. Needs scripts/install-armhf-cross-deps.sh
# to have run, and the Rust target installed.
#
# Usage (from the repository root):
#   scripts/build-cli.sh armv7-unknown-linux-gnueabihf <output directory>
#
# Writes <output directory>/jamuru-cli-linux-armv7.tar.gz and a .sha256 next to
# it. The archive holds `jamuru`, LICENSE and THIRD_PARTY_LICENSES.md.
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
    ;;
  *)
    echo "unsupported target: $target" >&2
    exit 2
    ;;
esac

export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_LIBDIR="/usr/lib/${triple}/pkgconfig:/usr/share/pkgconfig"
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER="${triple}-gcc"
export CC_armv7_unknown_linux_gnueabihf="${triple}-gcc"

cargo build --release --locked --bin jamuru --target "$target"

binary="${CARGO_TARGET_DIR:-target}/${target}/release/jamuru"
OBJDUMP="${triple}-objdump" scripts/check-glibc-floor.sh "$binary" "$glibc_floor"

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
mkdir -p "$staging/$name" "$out"
install -m 755 "$binary" "$staging/$name/jamuru"
"${triple}-strip" "$staging/$name/jamuru"
cp LICENSE THIRD_PARTY_LICENSES.md "$staging/$name/"
tar -C "$staging" -czf "$out/$name.tar.gz" "$name"
(cd "$out" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "wrote $out/$name.tar.gz"

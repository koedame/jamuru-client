#!/usr/bin/env bash
# Installs the Tauri CLI (`cargo tauri`) from the official prebuilt binary.
#
# `cargo install tauri-cli` compiles it from source on every run, which took
# 5-9 minutes of a release build. The Tauri project publishes the same binary
# on its GitHub releases; this downloads it, checks it against the pinned
# SHA-256 and puts it in ~/.cargo/bin.
#
# The version is pinned to the `tauri` crate in src-tauri/Cargo.lock. To move
# it, change the version and the four checksums below together (the digests are
# listed on the release: `gh api repos/tauri-apps/tauri/releases` -> assets).
#
# Usage:
#   scripts/install-tauri-cli.sh
set -euo pipefail

version=2.12.1

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)
    asset=cargo-tauri-x86_64-unknown-linux-gnu.tgz
    sha256=04ef3a9a2ed7b7dc479bef08ab40deee80065c99fc778f79ebce775eb72bbeff ;;
  Darwin-arm64)
    asset=cargo-tauri-aarch64-apple-darwin.zip
    sha256=ee35ee364a0c626df7852d9a5bc0c5394b6440772e9864cfb9b11dcb8d33f527 ;;
  Darwin-x86_64)
    asset=cargo-tauri-x86_64-apple-darwin.zip
    sha256=e5386ba6dadc3fbbdcf46c1f95b700f28caf03db979de50a29adb872022b4a57 ;;
  MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64)
    asset=cargo-tauri-x86_64-pc-windows-msvc.zip
    sha256=d3919ebe0bf7013dd98293862784dd37b8f76db2d650e13a3ec11c5dd799c519 ;;
  *)
    echo "no prebuilt Tauri CLI for $(uname -s)-$(uname -m); use: cargo install tauri-cli --version ${version} --locked" >&2
    exit 1 ;;
esac

bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

curl --fail --silent --show-error --location --retry 3 \
  --output "$work/$asset" \
  "https://github.com/tauri-apps/tauri/releases/download/tauri-cli-v${version}/${asset}"

if command -v sha256sum > /dev/null; then
  actual=$(sha256sum "$work/$asset" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$work/$asset" | cut -d' ' -f1)
fi
if [ "$actual" != "$sha256" ]; then
  echo "checksum mismatch for $asset: expected $sha256, got $actual" >&2
  exit 1
fi

mkdir -p "$bin_dir" "$work/out"
case "$asset" in
  *.tgz) tar -xzf "$work/$asset" -C "$work/out" ;;
  *) unzip -q "$work/$asset" -d "$work/out" ;;
esac
cp "$work"/out/cargo-tauri* "$bin_dir/"
chmod +x "$bin_dir"/cargo-tauri*

"$bin_dir"/cargo-tauri tauri --version

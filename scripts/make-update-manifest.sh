#!/usr/bin/env bash
# Writes `latest.json`, the file the installed apps read to learn that a newer
# release exists (ADR-041), and refuses to when the release would not update
# anyone.
#
# Usage (from the repository root, after the build jobs uploaded their bundles
# into <artifacts>, one directory per job as `actions/download-artifact` lays
# them out):
#   scripts/make-update-manifest.sh <tag> <artifacts> > latest.json
#
# An app installs an update only when all of these hold, so each is checked
# here, at release time, instead of failing on every user's machine:
#   - the version the bundles were built as (`src-tauri/tauri.conf.json`) is the
#     tag's version, or the app would offer the same release again forever;
#   - every platform has its updater bundle and signature;
#   - each signature names the version it was signed for, and it is that
#     version (the app rejects a signature that does not).
set -euo pipefail

tag=${1:?usage: $0 <tag> <artifacts directory>}
artifacts=${2:?usage: $0 <tag> <artifacts directory>}
repository=${GITHUB_REPOSITORY:-koedame/jamjam-client}
conf=src-tauri/tauri.conf.json

version=$(jq -r .version "$conf")
tag_version=${tag#v}
if [ "${tag_version%%-*}" != "$version" ]; then
  echo "ERROR: tag $tag is for ${tag_version%%-*} but $conf builds $version." >&2
  echo "Bump the version in $conf (and Cargo.toml) before tagging." >&2
  exit 1
fi

# Prints the only file matching <pattern>, or fails.
one_file() {
  local name=$1 pattern=$2 files=()
  shopt -s nullglob globstar
  # shellcheck disable=SC2206 # intentional glob expansion of the pattern string
  files=($pattern)
  shopt -u nullglob globstar
  if [ ${#files[@]} -ne 1 ]; then
    echo "ERROR: expected one $name ($pattern), found ${#files[@]}" >&2
    exit 1
  fi
  printf '%s\n' "${files[0]}"
}

platforms='{}'
add_platform() {
  local key=$1 pattern=$2 bundle signature comment
  bundle=$(one_file "$key bundle" "$pattern")
  signature=$(one_file "$key signature" "$bundle.sig")

  # A signature is the base64 of a minisign file; its trusted comment (covered
  # by the signature) carries `version:<the version it was signed for>`.
  comment=$(base64 -d < "$signature" | sed -n 's/^trusted comment: //p')
  if ! printf '%s' "$comment" | tr '\t' '\n' | grep -qxF "version:$version"; then
    echo "ERROR: $signature was not signed for version $version" >&2
    echo "trusted comment: '$comment'" >&2
    exit 1
  fi

  platforms=$(jq \
    --arg key "$key" \
    --arg url "https://github.com/$repository/releases/download/$tag/$(basename "$bundle")" \
    --rawfile signature "$signature" \
    '.[$key] = {url: $url, signature: ($signature | rtrimstr("\n"))}' <<< "$platforms")
}

add_platform darwin-aarch64-app "$artifacts/jamjam-macos-arm64/**/*.app.tar.gz"
add_platform darwin-x86_64-app "$artifacts/jamjam-macos-x64/**/*.app.tar.gz"
add_platform linux-x86_64-appimage "$artifacts/jamjam-linux-x64/**/*.AppImage"
add_platform windows-x86_64-nsis "$artifacts/jamjam-windows-x64/**/*-setup.exe"
add_platform windows-x86_64-msi "$artifacts/jamjam-windows-x64/**/*.msi"

jq -n \
  --arg version "$version" \
  --arg pub_date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson platforms "$platforms" \
  '{version: $version, pub_date: $pub_date, platforms: $platforms}'

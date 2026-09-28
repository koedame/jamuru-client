#!/usr/bin/env bash
# Deletes the install-only packages of every beta release older than <tag>,
# so a previous beta cannot be downloaded and installed once a newer one has
# shipped (the current beta, <tag>, is left alone).
#
# Usage: scripts/prune-beta-packages.sh <tag>     (needs `gh` and a token)
#
# Only macOS's `.dmg`, Windows's `.msi` and Linux's `.deb` (and a `.sig` next
# to one of them, when the bundler made one) are deleted: none of the three
# can replace itself (REQ-UPD-004), so nothing reads them again once a newer
# beta exists. Everything else on an old beta release stays:
#   - `*.app.tar.gz`, `*.AppImage`, `*-setup.exe` and their `.sig` files are
#     what the app's own updater fetches to move an installed beta forward,
#     and what scripts/publish-beta-channel-rollback.sh (ADR-057) points
#     still-outdated betas back to when a newer beta turns out bad. Deleting
#     them would break both.
#   - `latest.json` is what the rollback script reads from the old release.
# A non-beta tag (a release, no `-`) is left alone entirely: this only prunes
# betas.
#
# `GH` names the command that talks to GitHub (the tests give it a stand-in).
set -euo pipefail

tag=${1:?usage: $0 <tag>}
repository=${GITHUB_REPOSITORY:-koedame/jamjam-client}
gh=${GH:-gh}

case "$tag" in
  *-beta.*) ;;
  *)
    echo "$tag is not a beta; nothing to prune"
    exit 0
    ;;
esac

old_betas=$(
  "$gh" release list -R "$repository" --json tagName -L 200 --jq '.[].tagName' \
    | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+-beta\.[0-9]+$' \
    | grep -vxF "$tag" \
    || true
)

if [ -z "$old_betas" ]; then
  echo "no older beta releases to prune"
  exit 0
fi

while IFS= read -r old_tag; do
  [ -n "$old_tag" ] || continue

  assets=$("$gh" release view "$old_tag" -R "$repository" --json assets --jq '.assets[].name' || true)
  to_delete=$(printf '%s\n' "$assets" | grep -E '\.(dmg|msi|deb)(\.sig)?$' || true)
  [ -n "$to_delete" ] || continue

  while IFS= read -r asset; do
    [ -n "$asset" ] || continue
    echo "pruning $old_tag: $asset"
    "$gh" release delete-asset "$old_tag" "$asset" -R "$repository" --yes
  done <<< "$to_delete"
done <<< "$old_betas"

echo "done pruning packages older than $tag"

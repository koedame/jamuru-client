#!/usr/bin/env bash
# Rolls the beta update channel back to a past release, on purpose (ADR-057).
#
# Usage: scripts/publish-beta-channel-rollback.sh <release-tag>
#   e.g. scripts/publish-beta-channel-rollback.sh v0.1.0-beta.16
#
# scripts/publish-beta-channel.sh only ever moves beta-channel forward, so a
# late-finishing run cannot pull every beta back by accident. This script is
# the deliberate way around that guard, for when a release turns out to be
# bad and betas need to stop updating into it.
#
# It republishes the `latest.json` that <release-tag> already carries (built
# by scripts/make-update-manifest.sh at release time, so its signatures are
# already good for that version) as beta-channel's manifest, regardless of
# version order.
#
# `GH` names the command that talks to GitHub (the tests give it a stand-in).
set -euo pipefail

tag=${1:?usage: $0 <release-tag>}
repository=${GITHUB_REPOSITORY:-koedame/jamuru-client}
gh=${GH:-gh}
here=$(dirname "$0")

if ! "$gh" release view "$tag" -R "$repository" >/dev/null 2>&1; then
  echo "ERROR: no release $tag in $repository" >&2
  exit 1
fi

manifest_json=$("$gh" release download "$tag" -R "$repository" -p latest.json -O - 2>/dev/null) || true
if [ -z "$manifest_json" ]; then
  echo "ERROR: release $tag has no latest.json (built before ADR-041, or not an app release)" >&2
  exit 1
fi

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT
manifest="$workdir/latest.json"
printf '%s' "$manifest_json" > "$manifest"

version=$(jq -r .version "$manifest")
echo "rolling beta-channel back to $tag ($version)"

FORCE_ROLLBACK=1 "$here/publish-beta-channel.sh" "$manifest"

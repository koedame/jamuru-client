#!/usr/bin/env bash
# Puts an update manifest where the betas look for updates (ADR-045): the asset
# `latest.json` of the release `beta-channel`.
#
# Usage: scripts/publish-beta-channel.sh <latest.json>     (needs `gh` and a token)
#
# A beta installed from an app's own updater reads this file instead of
# `releases/latest`, which does not exist until there is a release that is not a
# pre-release. Both betas and releases are put here, so a beta moves on to the
# release of its version. The file only ever moves to a newer version: a beta of
# 0.1.0 built after 0.1.0 was released would otherwise pull every beta back.
#
# `GH` names the command that talks to GitHub (the tests give it a stand-in).
set -euo pipefail

manifest=${1:?usage: $0 <latest.json>}
repository=${GITHUB_REPOSITORY:-koedame/jamjam-client}
gh=${GH:-gh}
channel=beta-channel

new=$(jq -r .version "$manifest")

if ! "$gh" release view "$channel" -R "$repository" >/dev/null 2>&1; then
  "$gh" release create "$channel" -R "$repository" \
    --prerelease \
    --title "Beta update channel" \
    --notes "The update manifest the betas read. Not a release: the file is replaced by every beta and every release." \
    --target "${GITHUB_SHA:?GITHUB_SHA is needed to create the beta-channel release}"
fi

# No file yet, or one that is not a manifest, counts as nothing published.
current=$("$gh" release download "$channel" -R "$repository" -p latest.json -O - 2>/dev/null | jq -r '.version // empty' 2>/dev/null || true)

# `~` sorts before the end of a version, so 0.1.0~17 < 0.1.0, which is how
# semver puts a pre-release below its release.
if [ -n "$current" ] && ! dpkg --compare-versions "${new/-/\~}" gt "${current/-/\~}"; then
  echo "beta-channel already has $current; not replacing it with $new"
  exit 0
fi

"$gh" release upload "$channel" "$manifest" -R "$repository" --clobber
echo "beta-channel now has $new"

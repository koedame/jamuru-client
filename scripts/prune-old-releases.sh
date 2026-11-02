#!/usr/bin/env bash
# Deletes every release of the same channel as <tag> that is older than <tag>,
# so only the newest beta (and, once there is one, the newest stable release)
# stays on the releases page and an old build with a known flaw cannot be
# downloaded and installed (REQ-UPD-020, ADR-092).
#
# Usage: scripts/prune-old-releases.sh <tag>     (needs `gh` and a token)
#   DRY_RUN=1 prints what would be deleted and deletes nothing.
#
# - Channels: `vX.Y.Z-beta.N` is the beta channel, `vX.Y.Z` the stable one. A
#   tag of either kind only prunes its own channel. Any other tag prunes nothing.
# - Only releases with a lower version than <tag> are deleted, so a run that
#   finishes late never deletes a newer release.
# - The release `beta-channel` is not a version: it is where the betas read
#   their update manifest (ADR-045), and its name matches neither channel.
# - The release and its assets are deleted. The git tag stays: the update
#   drill builds "the release before" from it, and the servers are released
#   under the same tag.
# - A release whose assets the beta-channel manifest still points at is kept,
#   because installed betas would fetch their update from it.
#
# `GH` names the command that talks to GitHub (the tests give it a stand-in).
set -euo pipefail

tag=${1:?usage: $0 <tag>}
repository=${GITHUB_REPOSITORY:-koedame/jamuru-client}
gh=${GH:-gh}

beta_re='^v[0-9]+\.[0-9]+\.[0-9]+-beta\.[0-9]+$'
stable_re='^v[0-9]+\.[0-9]+\.[0-9]+$'

if printf '%s\n' "$tag" | grep -qE "$beta_re"; then
  channel_re=$beta_re
elif printf '%s\n' "$tag" | grep -qE "$stable_re"; then
  channel_re=$stable_re
else
  echo "$tag is not a beta or a stable release tag; nothing to prune"
  exit 0
fi

# Never delete anything on the strength of a release that is not there.
if ! "$gh" release view "$tag" -R "$repository" >/dev/null 2>&1; then
  echo "ERROR: no release $tag in $repository; not deleting anything" >&2
  exit 1
fi

# `sort -V` puts beta.9 before beta.10; what comes before <tag> is older.
older=$(
  { "$gh" release list -R "$repository" --json tagName -L 1000 --jq '.[].tagName' \
      | grep -E "$channel_re" | grep -vxF "$tag" || true
    echo "$tag"
  } | sort -uV | sed -n "/^${tag//./\\.}\$/q;p"
)

if [ -z "$older" ]; then
  echo "no releases older than $tag to delete"
  exit 0
fi

channel_manifest=$("$gh" release download beta-channel -R "$repository" -p latest.json -O - 2>/dev/null || true)

while IFS= read -r old_tag; do
  [ -n "$old_tag" ] || continue

  if printf '%s' "$channel_manifest" | grep -qF "/download/$old_tag/"; then
    echo "keeping $old_tag: the beta-channel manifest still points at it"
    continue
  fi

  if [ -n "${DRY_RUN:-}" ]; then
    echo "would delete release $old_tag"
  else
    echo "deleting release $old_tag"
    "$gh" release delete "$old_tag" -R "$repository" --yes
  fi
done <<< "$older"

echo "done deleting releases older than $tag"

#!/usr/bin/env bash
# Fail when a workflow uses a third-party action by a ref that can move.
#
# A branch or tag of someone else's repository can be rewritten, and whatever
# it then points to runs with the job's token and secrets. Third-party actions
# must be pinned to a full 40-character commit SHA (keep the version in a
# trailing `# vX.Y.Z` comment so Dependabot can update it), or a docker image
# digest. Actions from the `actions/` organization and local `./` actions are
# not checked.
#
# Usage: scripts/check-action-pins.sh [directory]   (default: .github)
set -euo pipefail

dir="${1:-.github}"
status=0

while IFS= read -r -d '' file; do
  while IFS= read -r line; do
    # "uses: owner/repo@ref" or "- uses: docker://image@ref", comments excluded
    case "$line" in
      *[![:space:]]*) ;;
      *) continue ;;
    esac
    spec=$(printf '%s\n' "$line" | sed -nE 's/^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]+["'"'"']?([^[:space:]"'"'"'#]+).*/\2/p')
    [ -n "$spec" ] || continue
    case "$spec" in
      ./*|actions/*) continue ;;
    esac
    ref="${spec##*@}"
    [ "$ref" != "$spec" ] || ref=""
    if printf '%s\n' "$ref" | grep -qE '^[0-9a-f]{40}$|^sha256:[0-9a-f]{64}$'; then
      continue
    fi
    echo "NOT PINNED: $file: $spec"
    status=1
  done < "$file"
done < <(find "$dir" -type f \( -name '*.yml' -o -name '*.yaml' \) -print0)

if [ "$status" -ne 0 ]; then
  echo
  echo "Pin each third-party action to a commit SHA and keep the version in a comment:"
  echo "  uses: owner/repo@<40 hex chars> # v1.2.3"
  exit 1
fi
echo "All third-party action references are pinned."

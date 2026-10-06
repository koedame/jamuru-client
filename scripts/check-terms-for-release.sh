#!/usr/bin/env bash
# Checks the terms and the pages the app opens before a stable release:
#   - the terms have a date of enactment (no "○月○日" left in docs/terms.md)
#   - every link to this repository's pages (the privacy page and the
#     announcements the app opens, the LICENSE the terms point at) is
#     answered with 200 where it is published
#
# Usage (from the repository root, with network access):
#   scripts/check-terms-for-release.sh
set -euo pipefail

failed=0

if grep -n '○月○日' docs/terms.md; then
  echo "FAIL: docs/terms.md has no date of enactment yet" >&2
  failed=1
fi

urls=$(grep -ohE 'https://github\.com/koedame/jamuru-client/blob/[^")> ]+' \
  src-tauri/src/terms.rs docs/terms.md | sort -u)
if [ -z "$urls" ]; then
  echo "FAIL: found no link to the repository's pages - the scan is not reading the terms" >&2
  exit 1
fi
for url in $urls; do
  code=$(curl -s -o /dev/null -L -w '%{http_code}' "$url" || true)
  if [ "$code" != "200" ]; then
    echo "FAIL: $url answered $code" >&2
    failed=1
  else
    echo "ok: $url"
  fi
done

exit "$failed"

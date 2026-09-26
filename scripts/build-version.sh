#!/usr/bin/env bash
# Prints the version an app is built as for a release tag (ADR-045).
#
#   vX.Y.Z          -> X.Y.Z
#   vX.Y.Z-beta.N   -> X.Y.Z-N
#
# A beta has to be a different version from the next beta, or the installed
# app never sees the newer one as newer. The pre-release part is only the
# number, because the Windows installer accepts nothing but a number there.
# It still sorts below X.Y.Z, so a beta moves to the release of its version.
set -euo pipefail

tag=${1:?usage: $0 <tag>}
version=${tag#v}

case "$version" in
  *-beta.*)
    number=${version##*-beta.}
    if ! [[ "$number" =~ ^[0-9]+$ ]] || [ "${version%-beta.*}" != "${version%%-*}" ]; then
      echo "ERROR: tag $tag is not vX.Y.Z-beta.N" >&2
      exit 1
    fi
    printf '%s-%s\n' "${version%%-*}" "$number"
    ;;
  *-*)
    echo "ERROR: tag $tag is a pre-release other than -beta.N; only vX.Y.Z and vX.Y.Z-beta.N are built" >&2
    exit 1
    ;;
  *)
    printf '%s\n' "$version"
    ;;
esac

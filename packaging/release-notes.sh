#!/bin/sh
# Prints the section of CHANGELOG.md for a version, the release's notes
# (docs/design/release.md §8); fails when there is none.
#
#   packaging/release-notes.sh 0.3.0
set -eu
version=$1
cd "$(dirname "$0")/.."
[ -f CHANGELOG.md ] || { echo "release-notes.sh: no CHANGELOG.md" >&2; exit 1; }
# From "## [0.3.0]" or "## 0.3.0" to the next "## ".
notes=$(awk -v version="$version" '
    /^## / {
        if (found) exit
        heading = $2
        gsub(/[][]/, "", heading)
        if (heading == version) { found = 1; next }
    }
    found { print }
' CHANGELOG.md)
[ -n "$notes" ] || { echo "release-notes.sh: CHANGELOG.md has no section for $version" >&2; exit 1; }
printf '%s\n' "$notes"

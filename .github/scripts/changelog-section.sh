#!/bin/sh
# Prints the body of a version's section in CHANGELOG.md (without its heading), for the
# GitHub release notes. Usage: changelog-section.sh 2.11.0 [CHANGELOG.md]
# A missing section prints a warning and nothing, so the release still goes out.
set -eu
version="$1"
file="${2:-CHANGELOG.md}"
section=$(awk -v v="$version" '
  /^## \[/ {
    if (found) exit
    if (index($0, "## [" v "]") == 1) { found = 1; next }
  }
  /^\[[^]]+\]: / { if (found) exit }
  found { print }
' "$file" | sed -e '/./,$!d')
if [ -z "$section" ]; then
  echo "::warning::CHANGELOG.md has no section for $version" >&2
  exit 0
fi
printf '%s\n' "$section"

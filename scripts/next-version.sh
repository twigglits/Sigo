#!/usr/bin/env bash
# Compute the next release version from conventional commits.
#
# Usage: scripts/next-version.sh [auto|patch|minor|major]   (default: auto)
# Prints the bare next version (e.g. "0.2.0") on stdout; rationale on stderr.
#
# Auto rules over commits since the last v* tag:
#   - breaking change ("type!:" subject or "BREAKING CHANGE" in body) -> major,
#     EXCEPT while still on 0.x, where breaking bumps minor (Cargo's 0.x
#     semantics); 1.0.0 is only ever cut deliberately via the "major" override.
#   - any feat -> minor
#   - anything else (fix, perf, docs, chore, ci, refactor, test, ...) -> patch
#
# Guards: refuses to run if the workspace Cargo.toml version does not match the
# last tag (the two must never drift), or if there is nothing to release.
set -euo pipefail

bump="${1:-auto}"
case "$bump" in
  auto|patch|minor|major) ;;
  *) echo "usage: $0 [auto|patch|minor|major]" >&2; exit 2 ;;
esac

cargo_version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
[ -n "$cargo_version" ] || { echo "error: no version in Cargo.toml" >&2; exit 1; }

last_tag=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)
if [ -n "$last_tag" ]; then
  if [ "${last_tag#v}" != "$cargo_version" ]; then
    echo "error: Cargo.toml version ($cargo_version) != last tag ($last_tag); fix the drift before releasing" >&2
    exit 1
  fi
  range="$last_tag..HEAD"
else
  range="HEAD"
fi

count=$(git rev-list --count "$range")
if [ "$count" -eq 0 ]; then
  echo "error: no commits since $last_tag — nothing to release" >&2
  exit 1
fi

IFS=. read -r major minor patch <<<"$cargo_version"

if [ "$bump" = "auto" ]; then
  bump="patch"
  if git log --format=%s "$range" | grep -Eq '^[a-z]+(\([^)]*\))?!:'; then
    bump="major"
  elif git log --format=%B "$range" | grep -Eq '^BREAKING[ -]CHANGE:'; then
    bump="major"
  elif git log --format=%s "$range" | grep -Eq '^feat(\([^)]*\))?:'; then
    bump="minor"
  fi
  if [ "$bump" = "major" ] && [ "$major" -eq 0 ]; then
    echo "note: breaking change on 0.x bumps minor; use an explicit 'major' argument to cut 1.0.0" >&2
    bump="minor"
  fi
  echo "auto: $count commit(s) since ${last_tag:-the beginning} -> $bump bump" >&2
fi

case "$bump" in
  major) next="$((major + 1)).0.0" ;;
  minor) next="$major.$((minor + 1)).0" ;;
  patch) next="$major.$minor.$((patch + 1))" ;;
esac

echo "$next"

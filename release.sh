#!/usr/bin/env bash
#
# release.sh — cut a new env-wizard release.
#
#   ./release.sh <X.Y.Z> [--yes]
#
# Bumps the version in Cargo.toml (+ Cargo.lock), moves the CHANGELOG's
# [Unreleased] section to the new version, rewrites the pinned download URLs in
# the README, commits, tags `vX.Y.Z`, and pushes main + the tag. The GitHub
# Actions release workflow then builds the binaries, publishes the release, and
# bumps the Homebrew tap.
#
# Requires a clean working tree on `main`, in sync with origin. Set $CARGO to
# point at a specific cargo if it isn't on PATH.

set -euo pipefail

CARGO="${CARGO:-cargo}"
ASSUME_YES=0
VERSION=""

for arg in "$@"; do
  case "$arg" in
    --yes | -y) ASSUME_YES=1 ;;
    *) VERSION="$arg" ;;
  esac
done

die() {
  echo "error: $*" >&2
  exit 1
}

# --- validate arguments -----------------------------------------------------
[ -n "$VERSION" ] || die "usage: ./release.sh <X.Y.Z> [--yes]"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must be semver X.Y.Z (got '$VERSION')"

TAG="v$VERSION"
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

# --- preflight checks -------------------------------------------------------
[ -n "$(git rev-parse --show-toplevel 2>/dev/null)" ] || die "not a git repo"
[ "$(git rev-parse --abbrev-ref HEAD)" = "main" ] || die "must be on the 'main' branch"
[ -z "$(git status --porcelain)" ] || die "working tree is dirty — commit or stash first"

git fetch --quiet origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "local main is not in sync with origin/main"

git rev-parse "$TAG" >/dev/null 2>&1 && die "tag $TAG already exists"

OLD="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/^version = "(.*)"/\1/')"
[ -n "$OLD" ] || die "could not read current version from Cargo.toml"
echo "Releasing $OLD → $VERSION ($TAG)"

# --- edits ------------------------------------------------------------------
# 1) Cargo.toml package version (line-anchored, so deps are untouched).
perl -0pi -e "s/^version = \"\\d+\\.\\d+\\.\\d+\"/version = \"$VERSION\"/m" Cargo.toml

# 2) Refresh Cargo.lock.
"$CARGO" build --quiet

# 3) CHANGELOG: insert a dated section under [Unreleased] and update links.
DATE="$(date +%F)"
perl -0pi -e "s/^## \\[Unreleased\\]\\n/## [Unreleased]\n\n## [$VERSION] - $DATE\n/m" CHANGELOG.md
perl -0pi -e "s{^\\[Unreleased\\]: .*/compare/v.*\\.\\.\\.HEAD\$}{[Unreleased]: https://github.com/alphonse-terrier/env-wizard/compare/v$VERSION...HEAD\n[$VERSION]: https://github.com/alphonse-terrier/env-wizard/compare/v$OLD...v$VERSION}m" CHANGELOG.md

# 4) README: bump pinned download URLs from the old version to the new one.
perl -0pi -e "s/v$OLD/v$VERSION/g" README.md

# --- review -----------------------------------------------------------------
echo
git --no-pager diff --stat
echo
if [ "$ASSUME_YES" -ne 1 ]; then
  read -r -p "Commit, tag $TAG, and push? [y/N] " reply
  case "$reply" in
    [yY] | [yY][eE][sS]) ;;
    *) die "aborted — changes left uncommitted" ;;
  esac
fi

# --- commit, tag, push ------------------------------------------------------
git add Cargo.toml Cargo.lock CHANGELOG.md README.md
git commit -m "Release $TAG"
git tag -a "$TAG" -m "env-wizard $VERSION"
git push origin main
git push origin "$TAG"

echo
echo "✓ Pushed $TAG. Watch the release build at:"
echo "  https://github.com/alphonse-terrier/env-wizard/actions"

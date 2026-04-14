#!/usr/bin/env bash
set -euo pipefail

# Ensure working tree is clean
if [ -n "$(git status --porcelain)" ]; then
  echo "Error: working tree is dirty — commit or stash changes first." >&2
  exit 1
fi

# Show current tags for reference
latest=$(git describe --tags --abbrev=0 2>/dev/null || echo "none")
echo "Latest tag: $latest"

# Prompt for new version
read -rp "New version (e.g. v0.1.0): " version
if [[ ! "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Error: version must match vX.Y.Z" >&2
  exit 1
fi

# Confirm
echo "Will tag $version and push to origin."
read -rp "Continue? [y/N] " confirm
if [[ "$confirm" != [yY] ]]; then
  echo "Aborted."
  exit 0
fi

git tag "$version"
git push origin "$version"

echo "Done — GitHub Actions will build the release."

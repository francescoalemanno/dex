#!/usr/bin/env bash
set -euo pipefail

# Ensure working tree is clean
if [ -n "$(git status --porcelain)" ]; then
  echo "Error: working tree is dirty — commit or stash changes first." >&2
  exit 1
fi
git push
# Show current tags for reference
latest=$(git tag --sort=-v:refname | head -n1)
latest=${latest:-none}
echo "Latest tag: $latest"

# Prompt for new version
read -rp "New version (e.g. v0.1.0): " version
if [[ ! "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Error: version must match vX.Y.Z" >&2
  exit 1
fi

# Strip the 'v' prefix for Cargo.toml
cargo_version="${version#v}"

# Update Cargo.toml version
if ! sed -i '' "s/^version = \"[0-9]\+\.[0-9]\+\.[0-9]\+\"$/version = \"$cargo_version\"/" Cargo.toml 2>/dev/null; then
  sed -i "s/^version = \"[0-9]\+\.[0-9]\+\.[0-9]\+\"$/version = \"$cargo_version\"/" Cargo.toml
fi

# Commit the version bump
if [ -n "$(git status --porcelain Cargo.toml)" ]; then
  git add Cargo.toml
  git commit -m "chore: bump version to $cargo_version"
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

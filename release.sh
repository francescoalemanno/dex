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

# Update Cargo.toml version in a POSIX-compatible, cross-platform way
CargoTmp=$(mktemp)

if ! awk -v new_version="$cargo_version" '
  /^\[package\]/ { in_package = 1 }
  in_package && /^\[/ && !/^\[package\]/ { in_package = 0 }

  in_package && $0 ~ /^[[:space:]]*version[[:space:]]*=[[:space:]]*"[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*"[[:space:]]*$/ {
    sub(/"[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*"/, "\"" new_version "\"", $0)
    changed = 1
  }

  { print }
  END {
    if (!changed) {
      exit 1
    }
  }
' Cargo.toml > "$CargoTmp"; then
  rm -f "$CargoTmp"
  echo "Error: failed to update Cargo.toml version." >&2
  exit 1
fi

if ! mv "$CargoTmp" Cargo.toml; then
  rm -f "$CargoTmp"
  echo "Error: failed to write Cargo.toml." >&2
  exit 1
fi

# Refresh Cargo.lock so --locked CI runs succeed after release updates.
if ! cargo generate-lockfile; then
  echo "Error: failed to regenerate Cargo.lock." >&2
  echo "Run \`cargo generate-lockfile\` manually and retry." >&2
  exit 1
fi

# Commit the version bump
if [ -n "$(git status --porcelain Cargo.toml Cargo.lock)" ]; then
  git add Cargo.toml Cargo.lock
  git commit -m "chore: bump version to $cargo_version"
fi

# Confirm
echo "Will tag $version and push to origin."
read -rp "Continue? [y/N] " confirm
if [[ "$confirm" != [yY] ]]; then
  echo "Aborted."
  exit 0
fi
git push
git tag "$version"
git push origin "$version"

echo "Done — GitHub Actions will build the release."

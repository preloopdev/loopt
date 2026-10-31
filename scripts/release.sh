#!/usr/bin/env bash
# Cut a Loopt release: bump the version, run the local gates, tag, push, and
# publish a host-built binary to the GitHub release.
#
#   scripts/release.sh 0.1.0
#
# The tag push also runs .github/workflows/release.yml, which builds the
# runner-platform binary and uploads it to the same release.
set -euo pipefail

version="${1:?usage: scripts/release.sh X.Y.Z}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "release: version must be X.Y.Z" >&2
  exit 2
}

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
tag="v$version"

[[ -z "$(git status --porcelain)" ]] || {
  echo "release: working tree is not clean" >&2
  exit 1
}
git rev-parse --verify --quiet "$tag" >/dev/null && {
  echo "release: tag $tag already exists" >&2
  exit 1
}

current="$(awk -F'"' '/^version = / { print $2; exit }' Cargo.toml)"
if [[ "$current" != "$version" ]]; then
  sed -i.bak "0,/^version = \".*\"/s//version = \"$version\"/" Cargo.toml
  rm -f Cargo.toml.bak
  cargo check --quiet
  git add Cargo.toml Cargo.lock
  git commit -m "chore(release): $tag"
fi

cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --lib --tests

git tag -a "$tag" -m "$tag"
git push origin main
git push origin "$tag"

cargo build --locked --release
mkdir -p dist
asset="dist/loopt-${tag}-$(uname -s | tr '[:upper:]' '[:lower:]')-$(uname -m).tar.gz"
tar -czf "$asset" -C target/release loopt

if gh release view "$tag" >/dev/null 2>&1; then
  gh release upload "$tag" "$asset" --clobber
else
  gh release create "$tag" --title "$tag" --generate-notes "$asset"
fi

echo "release: $tag published with $asset"

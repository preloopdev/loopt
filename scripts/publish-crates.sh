#!/usr/bin/env bash
# Publish the shared MIT crates in dependency order.
#
# Requires `cargo login` (or CARGO_REGISTRY_TOKEN) with publish-new and
# publish-update scopes.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

for crate in \
  preloop-gha-expressions \
  preloop-gha-authoring \
  preloop-gha-lint \
  preloop-gha-sec \
  loopt-sdk
do
  echo "publish: $crate"
  cargo publish -p "$crate"
  # crates.io indexes a new version asynchronously; the next crate depends
  # on the version just uploaded.
  sleep 15
done

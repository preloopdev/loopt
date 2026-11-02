#!/usr/bin/env bash
# Publish the authoring SDKs to their registries.
#
#   scripts/publish-sdks.sh
#
# Requires:
#   npm:   `npm login` (or NODE_AUTH_TOKEN) with publish rights on @loopt —
#          the first publish creates the package and must be run by an owner
#          of the @loopt npm organization.
#   cargo: `cargo login` (or CARGO_REGISTRY_TOKEN) with publish-new and
#          publish-update scopes.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

echo "publish: @loopt/sdk (npm)"
(cd sdk/typescript && npm publish --access public)

echo "publish: loopt-sdk (crates.io)"
cargo publish -p loopt-sdk

echo "done"

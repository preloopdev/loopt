#!/usr/bin/env bash
# Publish the authoring SDKs to their registries.
#
#   scripts/publish-sdks.sh
#
# Requires:
#   npm:   `npm login` (or NODE_AUTH_TOKEN) with publish rights on @preloopdev —
#          the first publish creates the package and must be run by an owner
#          of the @preloopdev npm organization.
#   cargo: `cargo login` (or CARGO_REGISTRY_TOKEN) with publish-new and
#          publish-update scopes. crates.io has no org scoping; after the
#          first publish the crate is handed to the preloopdev GitHub team
#          below (substitute your team's slug).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

echo "publish: @preloopdev/sdk (npm)"
(cd sdk/typescript && npm publish --access public)

echo "publish: loopt-sdk (crates.io)"
cargo publish -p loopt-sdk
# Hand crates.io ownership to the preloopdev org's team (name it as
# github:preloopdev:<team-slug>; adjust if the team is not "loopt").
cargo owner --add github:preloopdev:loopt loopt-sdk || \
  echo "note: add the crates.io team owner manually (cargo owner --add github:preloopdev:<team> loopt-sdk)"

echo "done"

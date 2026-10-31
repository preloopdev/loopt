# Vendored preloop-gha-parser authoring module

This directory contains an unmodified copy of the workflow authoring module
from the Preloop repository, which is MIT licensed.

- Upstream project: https://github.com/preloopdev/preloop
- Upstream revision: `0e19bada465663e9c89bc575809bbed67fde2c09` plus the
  authoring changes on branch `feat/loopt-authoring`
- Original paths:
  - `crates/preloop-gha-parser/src/authoring.rs`
  - `crates/preloop-gha-parser/LICENSE` (MIT)
- Vendored file: `src/authoring.rs` (byte-for-byte identical)
- Local additions: `src/lib.rs` (module declaration and re-exports only),
  `Cargo.toml` (standalone dependency list), this provenance note, and the
  upstream MIT license.

Loopt does not depend on the Preloop control plane, runner, or server. This is
a source distribution of the shared authoring API only.

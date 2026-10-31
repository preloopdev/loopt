# Vendored preloop-gha-parser authoring module

This directory contains an unmodified copy of Preloop's MIT-licensed workflow
authoring module.

Preloop's control plane is a separate, non-public product. Its source
repository is intentionally not linked from this public project. The shared
source is redistributed here under the MIT license; Loopt does not depend on
the Preloop control plane, runner, or server.

- Source revision: `0e19bada465663e9c89bc575809bbed67fde2c09` plus the
  authoring changes on `feat/loopt-authoring`
- Original paths:
  - `crates/preloop-gha-parser/src/authoring.rs`
  - `crates/preloop-gha-parser/LICENSE` (MIT)
- Vendored file: `src/authoring.rs` (byte-for-byte identical)
- Local additions: `src/lib.rs` (module declaration, re-exports, and the
  engine-provided secret-name list), `Cargo.toml`, this provenance note, and
  the MIT license.

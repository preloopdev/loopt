# Vendored preloop-gha-sec

Unmodified source from Preloop's MIT-licensed security audit crate.

Preloop's control plane is a separate, non-public product. Its source
repository is intentionally not linked from this public project. The shared
source is redistributed here under the MIT license.

- Source revision: `3339642ee`
- Original path: `crates/preloop-gha-sec/src/`
- Vendored path: `src/`
- Changes: `Cargo.toml` and path dependencies only; the Rust sources are
  byte-for-byte upstream. Upstream tests are not vendored.

The audit rules are an independent MIT implementation informed by the public
zizmor audit documentation and fixtures; zizmor is not linked, invoked, or
required at runtime.

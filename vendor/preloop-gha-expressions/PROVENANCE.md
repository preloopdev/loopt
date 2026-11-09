# Vendored preloop-gha-expressions

Unmodified source from Preloop's MIT-licensed expression engine.

Preloop's control plane is a separate, non-public product. Its source
repository is intentionally not linked from this public project. The shared
source is redistributed here under the MIT license.

- Source revision: `3339642ee`
- Original path: `crates/preloop-gha-expressions/src/`
- Vendored path: `src/`
- Changes: `Cargo.toml` and module wiring only, plus loopt-local changes in
  `src/lib.rs` and `src/evaluator.rs` (`collect_context_paths`); the remaining
  Rust sources are byte-for-byte upstream. Upstream tests are not vendored.

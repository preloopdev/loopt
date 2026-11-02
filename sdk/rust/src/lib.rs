//! `loopt-sdk`: a Rust authoring SDK for GitHub Actions workflows, mirroring
//! `sdk/typescript`.
//!
//! `loopt import --language rust` (and `loopt init --language rust`) vendor
//! this crate's source verbatim into every generated project's
//! `.loopt/sdk/rust`, so a generated `<stem>/Cargo.toml` can depend on it
//! with a plain relative path -- no publish step, no version to keep in
//! sync, and every generated project in a repository shares the exact
//! source the `loopt` binary that generated it was built from.
//!
//! Three pieces, in increasing order of abstraction:
//!
//! - [`expr`]: `${{ ... }}` expressions -- literals, operators, built-in
//!   functions, and dynamically navigable contexts (`github`, `env`, ...).
//! - [`core`]: hand-written `run`/`uses` step, `job`, and `workflow`
//!   constructors for the fields real workflows use, plus [`core::JobRef`]/
//!   [`core::StepRef`] typed references to `needs.<id>.outputs.<name>` and
//!   `steps.<id>.outputs.<name>`.
//! - [`emit`]: writes the assembled `serde_json::Value` workflow document as
//!   one JSON line on stdout, the shape `loopt gen`'s YAML backend consumes.

pub mod core;
pub mod emit;
pub mod error;
pub mod expr;

pub use error::{Error, Result};

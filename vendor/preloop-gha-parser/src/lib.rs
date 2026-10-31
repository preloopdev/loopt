//! Source-preserving GitHub Actions authoring document.
//!
//! Vendored from the MIT-licensed Preloop parser so Loopt can build without
//! access to the private Preloop repository. See PROVENANCE.md for the exact
//! upstream revision and original path. The authoring implementation is kept
//! byte-for-byte identical to upstream.

mod authoring;

pub use authoring::{DocumentError, WorkflowDocument};

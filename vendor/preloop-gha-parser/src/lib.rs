//! Source-preserving GitHub Actions authoring document.
//!
//! Vendored from the MIT-licensed Preloop parser so Loopt can build without
//! access to the private Preloop repository. See PROVENANCE.md for the exact
//! upstream revision and original path. The authoring implementation is kept
//! byte-for-byte identical to upstream.

mod authoring;

pub use authoring::{DocumentError, WorkflowDocument};

/// Tokens the execution engine mints per job and injects into every step.
/// Kept here because the vendored authoring parser intentionally omits the
/// execution-plan modules that define this list upstream.
pub const ENGINE_PROVIDED_SECRETS: &[&str] = &[
    "GITHUB_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
    "ACTIONS_RUNTIME_TOKEN",
    "ACTIONS_RUNTIME_URL",
    "ACTIONS_CACHE_URL",
    "ACTIONS_RESULTS_URL",
];

//! Vendored copy of the TypeScript authoring SDK (`sdk/typescript`), written
//! under `.loopt/sdk/typescript` so generated helpers (action bindings,
//! `secrets.ts`) run offline with no npm install.
//!
//! Sources are embedded in this binary at compile time via `include_str!`,
//! mirroring how `languages::rust` vendors `loopt-sdk` to `.loopt/sdk/rust`.
//! The vendored copy therefore always matches the `loopt` binary that wrote
//! it — upgrading `loopt` upgrades the vendored SDK on the next write.

use std::path::Path;

use anyhow::{Context, Result};

/// Project-relative directory the vendored SDK is written to. Generated
/// helpers import it with a relative specifier: `.loopt/actions/*.ts` uses
/// `../sdk/typescript/index.ts`, `.loopt/secrets.ts` uses
/// `./sdk/typescript/index.ts`.
pub const SDK_VENDOR_DIR: &str = ".loopt/sdk/typescript";

/// The SDK's `src/` tree, embedded at compile time. Paths are relative to
/// `SDK_VENDOR_DIR`. Internal imports inside the SDK use explicit `.ts`
/// relative specifiers, so the vendored tree needs no `package.json`.
const SDK_FILES: &[(&str, &str)] = &[
    ("index.ts", include_str!("../sdk/typescript/src/index.ts")),
    (
        "kit/emit.ts",
        include_str!("../sdk/typescript/src/kit/emit.ts"),
    ),
    (
        "kit/errors.ts",
        include_str!("../sdk/typescript/src/kit/errors.ts"),
    ),
    (
        "kit/expr.ts",
        include_str!("../sdk/typescript/src/kit/expr.ts"),
    ),
    (
        "kit/extra.ts",
        include_str!("../sdk/typescript/src/kit/extra.ts"),
    ),
    (
        "kit/index.ts",
        include_str!("../sdk/typescript/src/kit/index.ts"),
    ),
    (
        "kit/json.ts",
        include_str!("../sdk/typescript/src/kit/json.ts"),
    ),
    (
        "expr/contexts.ts",
        include_str!("../sdk/typescript/src/expr/contexts.ts"),
    ),
    (
        "expr/functions.ts",
        include_str!("../sdk/typescript/src/expr/functions.ts"),
    ),
    (
        "expr/index.ts",
        include_str!("../sdk/typescript/src/expr/index.ts"),
    ),
    (
        "expr/literal.ts",
        include_str!("../sdk/typescript/src/expr/literal.ts"),
    ),
    (
        "expr/operators.ts",
        include_str!("../sdk/typescript/src/expr/operators.ts"),
    ),
    (
        "core/index.ts",
        include_str!("../sdk/typescript/src/core/index.ts"),
    ),
    (
        "core/inputs.ts",
        include_str!("../sdk/typescript/src/core/inputs.ts"),
    ),
    (
        "core/job_ref.ts",
        include_str!("../sdk/typescript/src/core/job_ref.ts"),
    ),
    (
        "core/job_refs.ts",
        include_str!("../sdk/typescript/src/core/job_refs.ts"),
    ),
    (
        "core/job.ts",
        include_str!("../sdk/typescript/src/core/job.ts"),
    ),
    (
        "core/matrix.ts",
        include_str!("../sdk/typescript/src/core/matrix.ts"),
    ),
    (
        "core/normalize.ts",
        include_str!("../sdk/typescript/src/core/normalize.ts"),
    ),
    (
        "core/passes.ts",
        include_str!("../sdk/typescript/src/core/passes.ts"),
    ),
    (
        "core/ref_tokens.ts",
        include_str!("../sdk/typescript/src/core/ref_tokens.ts"),
    ),
    (
        "core/step_ref.ts",
        include_str!("../sdk/typescript/src/core/step_ref.ts"),
    ),
    (
        "core/step_refs.ts",
        include_str!("../sdk/typescript/src/core/step_refs.ts"),
    ),
    (
        "core/steps.ts",
        include_str!("../sdk/typescript/src/core/steps.ts"),
    ),
    (
        "core/workflow.ts",
        include_str!("../sdk/typescript/src/core/workflow.ts"),
    ),
    (
        "generated/action.ts",
        include_str!("../sdk/typescript/src/generated/action.ts"),
    ),
    (
        "generated/aliases.ts",
        include_str!("../sdk/typescript/src/generated/aliases.ts"),
    ),
    (
        "generated/model.ts",
        include_str!("../sdk/typescript/src/generated/model.ts"),
    ),
    (
        "generated/workflow.ts",
        include_str!("../sdk/typescript/src/generated/workflow.ts"),
    ),
];

/// The default import specifier for a generated TypeScript file living at
/// `relative` (e.g. `.loopt/actions/checkout.ts` → `../sdk/typescript/index.ts`).
///
/// The specifier is relative to the *file's* directory, not the repo root —
/// `.loopt/actions` and `.loopt` itself are exactly one level away, so the
/// two call sites need `../` and `./` respectively.
pub fn default_import_for(file_relative: &str) -> String {
    let depth = file_relative.matches('/').count();
    if depth <= 1 {
        return "./sdk/typescript/index.ts".to_string();
    }
    format!("{}sdk/typescript/index.ts", "../".repeat(depth - 1))
}

/// Write the vendored SDK under `root`, replacing only files whose contents
/// changed. Returns the number of files written.
///
/// Every write goes through `paths::write_atomic`, so symlinked parents or
/// destinations are rejected the same way as for any other generated file.
pub fn vendor(root: &Path) -> Result<usize> {
    let mut written = 0;
    for (relative, content) in SDK_FILES {
        let path = Path::new(SDK_VENDOR_DIR).join(relative);
        let unchanged = std::fs::read(root.join(&path))
            .map(|existing| existing == content.as_bytes())
            .unwrap_or(false);
        if unchanged {
            continue;
        }
        crate::paths::write_atomic(root, &path, content.as_bytes())
            .with_context(|| format!("vendor {}", path.display()))?;
        written += 1;
    }
    Ok(written)
}

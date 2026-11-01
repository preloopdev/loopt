//! Code generator for the TypeScript authoring SDK.
//!
//! The generator reads the embedded GitHub Actions workflow and action schemas
//! through [`preloop_gha_schema`] and renders four TypeScript modules:
//!
//! - `model.ts`: the schema model types plus both schemas as JSON literals.
//! - `aliases.ts`: camelCase property aliases for every mapping definition.
//! - `workflow.ts`: one TypeScript type per workflow schema definition.
//! - `action.ts`: one TypeScript type per action schema definition.
//!
//! Rendering is deterministic: the same schema always produces byte-identical
//! output in the same order.

#![forbid(unsafe_code)]

mod naming;
mod typescript;

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};

/// One generated file, with its content and its path relative to the output
/// directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    /// Path relative to the output directory, for example `model.ts`.
    pub path: String,

    /// Complete file contents, including the generated-file header.
    pub content: String,
}

/// Renders every generated TypeScript file.
///
/// The returned vector is in a stable order (`model.ts`, `aliases.ts`,
/// `workflow.ts`, `action.ts`), and every call produces byte-identical content
/// for the same embedded schemas.
///
/// # Errors
///
/// Returns an error when the schema cannot be serialized or a definition name
/// cannot be represented in TypeScript without colliding with another name or
/// a kit import.
pub fn generate_typescript() -> Result<Vec<GeneratedFile>> {
    typescript::generate()
}

/// Writes every generated file under `out_dir`, creating the directory and any
/// missing parents.
///
/// # Errors
///
/// Returns an error when a directory cannot be created or a file cannot be
/// written.
pub fn write_all(out_dir: &Path, files: &[GeneratedFile]) -> Result<()> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("creating output directory `{}`", out_dir.display()))?;

    for file in files {
        let path = out_dir.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating directory `{}`", parent.display()))?;
        }
        fs::write(&path, file.content.as_bytes())
            .with_context(|| format!("writing `{}`", path.display()))?;
    }

    Ok(())
}

/// Returns the paths of the generated files whose on-disk content is missing or
/// differs from `files`.
///
/// A missing output directory reports every file as stale rather than failing.
/// The returned paths preserve the order of `files`.
///
/// # Errors
///
/// Returns an error when an existing file cannot be read.
pub fn stale_files(out_dir: &Path, files: &[GeneratedFile]) -> Result<Vec<String>> {
    let mut stale = Vec::new();

    for file in files {
        let path = out_dir.join(&file.path);
        match fs::read(&path) {
            Ok(contents) => {
                if contents.as_slice() != file.content.as_bytes() {
                    stale.push(file.path.clone());
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                stale.push(file.path.clone());
            }
            Err(error) => {
                return Err(
                    anyhow::Error::new(error).context(format!("reading `{}`", path.display()))
                );
            }
        }
    }

    Ok(stale)
}

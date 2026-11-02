//! Shared types and the language registry for Loopt's authoring tools.
//!
//! Each built-in language module (`typescript`, `python`, `go`, `rust`)
//! emits native source code for a parsed workflow value and describes how
//! to execute that source to synthesize a workflow value back out. Custom
//! languages implement the same two operations out-of-process; see
//! `crate::plugin`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod go;
pub mod python;
pub mod rust;
pub mod typescript;

/// A single file produced by a language's `emit`. `path` is project-relative
/// and must stay under `.loopt/workflows` (each caller's own `SOURCE_DIR`),
/// with one carve-out: a vendored, cross-project shared SDK a backend writes
/// once and reuses across every project of that language (currently the
/// Rust backend's `.loopt/sdk/rust`, see `src/languages/rust.rs`) may
/// instead stay under `.loopt/sdk` (`paths::SDK_DIR`). `import`/`init`
/// tolerate an identical repeat of such a file across multiple documents in
/// one run; that is the only case where two documents' `files` may share a
/// `path`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub content: String,
}

/// The full set of files produced for one workflow's authoring source.
/// `entrypoint` must be one of `files`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedSource {
    pub entrypoint: PathBuf,
    pub files: Vec<GeneratedFile>,
}

/// How to execute a generated (or hand-authored) source to synthesize a
/// workflow JSON value. Run directly, never through a shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

/// The four built-in authoring languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    TypeScript,
    Python,
    Go,
    Rust,
}

impl Language {
    pub const ALL: [Language; 4] = [
        Language::TypeScript,
        Language::Python,
        Language::Go,
        Language::Rust,
    ];

    /// The canonical id used in `--language`, `.loopt/manifest.json`, and
    /// the `loopt plugin` protocol's `describe` response.
    pub fn id(self) -> &'static str {
        match self {
            Language::TypeScript => "typescript",
            Language::Python => "python",
            Language::Go => "go",
            Language::Rust => "rust",
        }
    }

    /// Parse a built-in language id. Returns `None` for anything else,
    /// including custom plugin language ids.
    pub fn parse(id: &str) -> Option<Language> {
        match id {
            "typescript" => Some(Language::TypeScript),
            "python" => Some(Language::Python),
            "go" => Some(Language::Go),
            "rust" => Some(Language::Rust),
            _ => None,
        }
    }

    /// Source file extensions this language's entrypoint may use (without
    /// the leading dot). Rust's entrypoint is always named `main.rs`
    /// alongside a sibling `Cargo.toml`; see `detect_entrypoint`.
    pub fn source_extensions(self) -> &'static [&'static str] {
        match self {
            Language::TypeScript => &["ts"],
            Language::Python => &["py"],
            Language::Go => &["go"],
            Language::Rust => &["rs"],
        }
    }

    /// Detect a built-in language from an entrypoint path discovered on
    /// disk, used when `loopt gen` finds new source under
    /// `.loopt/workflows` that is not yet in the manifest.
    pub fn detect_entrypoint(path: &Path) -> Option<Language> {
        if path.file_name().and_then(|n| n.to_str()) == Some("main.rs")
            && path
                .parent()
                .is_some_and(|dir| dir.join("Cargo.toml").is_file())
        {
            return Some(Language::Rust);
        }
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("ts") => Some(Language::TypeScript),
            Some("py") => Some(Language::Python),
            Some("go") => Some(Language::Go),
            _ => None,
        }
    }

    /// Emit native authoring source for `workflow`, rooted at project-
    /// relative `stem` (e.g. `.loopt/workflows/ci`, no extension).
    pub fn emit(
        self,
        workflow: &serde_json::Value,
        stem: &Path,
    ) -> anyhow::Result<GeneratedSource> {
        match self {
            Language::TypeScript => typescript::emit(workflow, stem),
            Language::Python => python::emit(workflow, stem),
            Language::Go => go::emit(workflow, stem),
            Language::Rust => rust::emit(workflow, stem),
        }
    }

    /// Describe how to run `source` (an entrypoint path) from the
    /// repository root to synthesize its workflow JSON value on stdout.
    pub fn command(self, source: &Path) -> anyhow::Result<CommandSpec> {
        match self {
            Language::TypeScript => typescript::command(source),
            Language::Python => python::command(source),
            Language::Go => go::command(source),
            Language::Rust => rust::command(source),
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

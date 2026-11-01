//! Loopt: author GitHub Actions workflows in TypeScript, Python, Go, or
//! Rust, and generate standard workflow YAML from them.
//!
//! `loopt import` converts existing workflow YAML into editable authoring
//! source; `loopt gen` runs authoring source and writes workflow YAML back
//! out. Neither command runs automatically — there is no watcher, hook, or
//! daemon. See `.loopt/manifest.json` for the durable source/output map
//! both commands share.
//!
//! This crate is split into a library (this file) and a thin `src/main.rs`
//! binary so the pieces below are independently testable from `tests/`.

/// GitHub Actions metadata, pins and generated bindings.
pub mod actions;
/// Executes a generated or hand-authored source and parses its single JSON
/// workflow value from stdout.
pub mod exec;
/// `loopt gen`.
pub mod gen;
/// `loopt import`.
pub mod import;
/// `loopt init`.
pub mod init;
/// Shared language types (`GeneratedFile`, `GeneratedSource`,
/// `CommandSpec`) and the built-in language registry.
pub mod languages;
/// `loopt lint`: native correctness and security checks.
pub mod lint;
/// `.loopt/manifest.json`: the durable source/output map.
pub mod manifest;
/// Path safety and atomic-write helpers.
pub mod paths;
/// The `loopt plugin <language>` process protocol.
pub mod plugin;
/// Names-only secret inventory synchronization.
pub mod secrets;
/// `shellcheck` integration for workflow `run:` scripts.
pub mod shellcheck;

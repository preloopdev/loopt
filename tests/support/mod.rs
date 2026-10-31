//! Shared helpers for Loopt's end-to-end integration tests.
//!
//! Every test exercises the real `loopt` binary as a subprocess (never the
//! crate's internals directly) and the real vendored parser
//! (`preloop_gha_authoring::WorkflowDocument`) as the single oracle for both
//! "is this YAML representable at all" and "do two workflow documents mean
//! the same thing, in the same nested key order". Nothing here fabricates
//! expected output: every assertion is derived from parsing real files
//! written by real subprocess runs.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use preloop_gha_authoring::WorkflowDocument;
use serde_json::Value;

/// Optional extra `PATH` entry, supplied by the developer or CI when the
/// language toolchains are installed outside the inherited `PATH`.
const EXTRA_PATH_ENV: &str = "LOOPT_TEST_PATH";

/// A self-deleting directory under the OS temp dir. Hand-rolled instead of
/// pulling in a crate: the only thing needed is "unique dir, removed on
/// drop, even on panic".
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_nanos();
        let safe_label: String = label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let mut path = std::env::temp_dir();
        path.push(format!(
            "loopt-test-{safe_label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .unwrap_or_else(|e| panic!("create temp dir {}: {e}", path.display()));
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// An isolated repository root for one test: a fresh temp directory acting
/// as the implicit `--root` Loopt operates on (every `run`/`run_args` call
/// uses this directory as the subprocess's working directory).
pub struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    pub fn new(label: &str) -> Self {
        Self {
            dir: TempDir::new(label),
        }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn path(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root().join(rel)
    }

    pub fn exists(&self, rel: impl AsRef<Path>) -> bool {
        self.path(rel).exists()
    }

    /// Writes `content` at `rel` (relative to the repo root), creating
    /// parent directories as needed. Returns the absolute path written.
    pub fn write(&self, rel: impl AsRef<Path>, content: &str) -> PathBuf {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create dir {}: {e}", parent.display()));
        }
        fs::write(&path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        path
    }

    /// Creates a symlink at `rel` (relative to the repo root) pointing at
    /// `target` (an absolute path, typically outside the repo), for
    /// path-escape safety tests.
    pub fn symlink(&self, rel: impl AsRef<Path>, target: &Path) -> PathBuf {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create dir {}: {e}", parent.display()));
        }
        std::os::unix::fs::symlink(target, &path)
            .unwrap_or_else(|e| panic!("symlink {} -> {}: {e}", path.display(), target.display()));
        path
    }

    pub fn read(&self, rel: impl AsRef<Path>) -> String {
        let path = self.path(rel);
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// All regular-file paths under `rel`, relative to the repo root, in
    /// sorted order. A missing directory reads as empty (nothing created
    /// yet is a legitimate state, not an error).
    pub fn list_files(&self, rel: impl AsRef<Path>) -> BTreeSet<PathBuf> {
        let base = self.path(rel);
        let mut out = BTreeSet::new();
        walk_files(&base, &base, &mut out);
        out
    }

    /// Runs `loopt <args>` with this repo as the working directory.
    pub fn run(&self, args: &[&str]) -> LooptOutput {
        run_in(self.root(), args)
    }
}

fn walk_files(base: &Path, dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            walk_files(base, &path, out);
        } else if file_type.is_file() {
            if let Ok(rel) = path.strip_prefix(base) {
                out.insert(rel.to_path_buf());
            }
        }
    }
}

/// Captured result of a `loopt` subprocess invocation.
pub struct LooptOutput {
    pub status_code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl LooptOutput {
    pub fn assert_success(&self) -> &Self {
        assert!(
            self.success,
            "expected `loopt` to succeed, got exit {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.status_code, self.stdout, self.stderr
        );
        self
    }

    pub fn assert_failure(&self) -> &Self {
        assert!(
            !self.success,
            "expected `loopt` to fail, but it succeeded\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.stdout, self.stderr
        );
        self
    }
}

fn from_output(output: Output) -> LooptOutput {
    LooptOutput {
        status_code: output.status.code(),
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Path to the `loopt` binary under test, provided by Cargo for
/// integration tests in this package.
pub fn loopt_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_loopt"))
}

fn augmented_path() -> std::ffi::OsString {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut parts: Vec<PathBuf> = std::env::var_os(EXTRA_PATH_ENV)
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    parts.extend(std::env::split_paths(&existing));
    std::env::join_paths(parts).expect("join PATH")
}

/// Runs `loopt <args>` with `cwd` as the working directory (so default
/// `.github/workflows` / `.loopt` discovery resolves there) and an
/// augmented `PATH` so pinned toolchains (`cargo`) are reachable regardless
/// of the invoking shell's environment.
pub fn run_in(cwd: &Path, args: &[&str]) -> LooptOutput {
    let output = Command::new(loopt_bin())
        .args(args)
        .current_dir(cwd)
        .env("PATH", augmented_path())
        .output()
        .unwrap_or_else(|e| panic!("spawn loopt {:?} in {}: {e}", args, cwd.display()));
    from_output(output)
}

/// Parses `path` with the shared authoring parser: the single oracle this
/// suite uses both to classify a fixture (representable vs. not) and to
/// compare two workflow documents.
pub fn parse_document(path: &Path) -> Result<WorkflowDocument, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    WorkflowDocument::parse(&text).map_err(|e| e.to_string())
}

/// Deep-compares two workflow values, requiring object keys to appear in
/// the *same order*, not just the same set -- `serde_json::Value`'s own
/// `PartialEq` treats objects as unordered maps (even with the
/// `preserve_order` feature, `IndexMap`'s `Eq` impl ignores order), which
/// would let matrix-axis reordering or top-level key reordering silently
/// pass. Numbers are compared by their exact source text first, falling
/// back to numeric equality, so integers at the edge of the main crate's
/// (non-arbitrary-precision) `serde_json::Number` range are held to the
/// same standard rather than silently coerced through floats.
pub fn assert_order_preserving_eq(expected: &Value, actual: &Value) {
    compare(expected, actual, "$");
}

fn compare(expected: &Value, actual: &Value, path: &str) {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            let e_keys: Vec<&String> = e.keys().collect();
            let a_keys: Vec<&String> = a.keys().collect();
            assert_eq!(
                e_keys, a_keys,
                "key order/set mismatch at {path}: expected {e_keys:?}, got {a_keys:?}"
            );
            for key in e_keys {
                compare(&e[key], &a[key], &format!("{path}.{key}"));
            }
        }
        (Value::Array(e), Value::Array(a)) => {
            assert_eq!(
                e.len(),
                a.len(),
                "array length mismatch at {path}: expected {}, got {}",
                e.len(),
                a.len()
            );
            for (i, (ev, av)) in e.iter().zip(a.iter()).enumerate() {
                compare(ev, av, &format!("{path}[{i}]"));
            }
        }
        (Value::Number(e), Value::Number(a)) => {
            assert!(
                e.to_string() == a.to_string() || e == a,
                "number mismatch at {path}: expected {e}, got {a}"
            );
        }
        (e, a) => {
            assert_eq!(e, a, "value mismatch at {path}");
        }
    }
}

/// Recursively collects every `*.yml`/`*.yaml` file under the directory
/// named by the `PRELOOP_FIXTURES` environment variable, sorted for
/// deterministic test ordering. Symlinks are skipped (never followed), so
/// traversal can't loop or escape the fixtures tree. Panics with a clear
/// message if the env var is unset or the directory can't be read -- the
/// corpus tests that call this are `#[ignore]`d precisely because they need
/// this set explicitly, so failing loudly (rather than silently skipping)
/// is the right default when they *are* run.
pub fn discover_fixture_yaml_files() -> Vec<PathBuf> {
    let root = fixtures_root();
    let mut out = Vec::new();
    collect_yaml(&root, &mut out);
    out.sort();
    out
}

pub fn fixtures_root() -> PathBuf {
    let root = std::env::var("PRELOOP_FIXTURES").unwrap_or_else(|_| {
        panic!(
            "PRELOOP_FIXTURES must be set to the Preloop fixtures directory to run \
             the corpus tests, e.g. PRELOOP_FIXTURES=/path/to/dev__preloop__loopt-authoring/fixtures \
             cargo test --test corpus -- --ignored"
        )
    });
    let root = PathBuf::from(root);
    assert!(
        root.is_dir(),
        "PRELOOP_FIXTURES={} is not a directory",
        root.display()
    );
    root
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_yaml(&path, out);
        } else if file_type.is_file() && is_yaml(&path) {
            out.push(path);
        }
    }
}

fn is_yaml(path: &Path) -> bool {
    match path.extension().and_then(OsStr::to_str) {
        Some(ext) => ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"),
        None => false,
    }
}

/// Everything under `.loopt/workflows` created by an `import`, relative to
/// the repo root -- used to discover what a language actually emitted
/// without hardcoding its file layout.
pub fn generated_workflow_files(repo: &TestRepo) -> BTreeSet<PathBuf> {
    repo.list_files(".loopt/workflows")
}

/// A short, filesystem-safe name derived from a fixture's path relative to
/// the fixtures root, so error messages and temp-dir labels can identify
/// which fixture failed without leaking the whole host path.
pub fn fixture_label(fixtures_root: &Path, fixture: &Path) -> String {
    fixture
        .strip_prefix(fixtures_root)
        .unwrap_or(fixture)
        .to_string_lossy()
        .replace(['/', '\\'], "__")
}

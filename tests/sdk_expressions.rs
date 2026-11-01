//! Integration test for the hand-written TypeScript expression builders under
//! `sdk/typescript/src/expr/`.
//!
//! `sdk/typescript/test/fixtures/emit-expressions.ts` prints a JSON workflow
//! whose step environment values are built with the TypeScript expression
//! builders. The real `loopt lint --json` then parses every `${{ ... }}` body
//! with the Preloop expression engine, which acts as the cross-check oracle:
//! the `valid` expressions must produce no `preloop/expression-syntax` finding
//! and the deliberately malformed expression in `invalid` must.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Repository root: the root package's manifest directory owns `sdk/typescript/`.
const REPO_ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Fixture that prints a workflow document built from TypeScript expressions.
const FIXTURE: &str = "sdk/typescript/test/fixtures/emit-expressions.ts";

/// Rule the expression engine reports for a body it cannot parse.
const EXPRESSION_RULE: &str = "preloop/expression-syntax";

/// Minimum number of expressions the `valid` fixture must emit.
const MIN_VALID_EXPRESSIONS: usize = 40;

/// Distinguishes temp directories created within one test process.
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// A unique temporary directory removed when the value is dropped.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates a fresh directory tagged with `label`.
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "loopt-expressions-{label}-{}-{nanos}-{id}",
            std::process::id(),
        ));
        fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("create temp dir {}: {error}", path.display()));
        Self { path }
    }

    /// Absolute path of the directory.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Human-readable captured output for assertion messages.
fn describe(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

/// Runs the TypeScript fixture in `mode` and returns its printed JSON together
/// with the number of `${{` expression markers it contains.
fn emit_workflow(mode: &str) -> (String, usize) {
    let output = Command::new("node")
        .args([FIXTURE, mode])
        .current_dir(Path::new(REPO_ROOT))
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "failed to spawn `node`; Node.js 26 or newer is required to run the \
                 TypeScript expression fixture: {error}"
            )
        });
    assert!(
        output.status.success(),
        "the `{mode}` fixture failed ({}).\n{}",
        output.status,
        describe(&output),
    );
    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("the `{mode}` fixture printed non-UTF-8 output: {error}"));
    let markers = stdout.matches("${{").count();
    (stdout, markers)
}

/// Writes `text` as `.github/workflows/exprs.yml` under `dir`.
fn write_workflow(dir: &Path, text: &str) {
    let workflows = dir.join(".github/workflows");
    fs::create_dir_all(&workflows)
        .unwrap_or_else(|error| panic!("create {}: {error}", workflows.display()));
    let path = workflows.join("exprs.yml");
    fs::write(&path, text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

/// Runs `loopt lint --json` in `dir` and returns its captured output plus the
/// locations of every `preloop/expression-syntax` finding.
fn lint_expression_findings(dir: &Path) -> (Output, Vec<String>) {
    let output = Command::new(env!("CARGO_BIN_EXE_loopt"))
        .args(["lint", "--json"])
        .current_dir(dir)
        .output()
        .unwrap_or_else(|error| panic!("failed to spawn the `loopt` binary: {error}"));
    let reports: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "`loopt lint --json` did not print a JSON report: {error}\n{}",
                describe(&output),
            )
        });
    let reports = reports.as_array().unwrap_or_else(|| {
        panic!(
            "`loopt lint --json` did not print an array of file reports.\n{}",
            describe(&output),
        )
    });
    assert!(
        !reports.is_empty(),
        "`loopt lint --json` reported no files.\n{}",
        describe(&output),
    );
    let mut findings = Vec::new();
    for report in reports {
        let file = report["file"].as_str().unwrap_or("");
        let report_findings = report["findings"].as_array().unwrap_or_else(|| {
            panic!(
                "the lint report for {file} has no findings array.\n{}",
                describe(&output),
            )
        });
        for finding in report_findings {
            if finding["rule_id"].as_str() == Some(EXPRESSION_RULE) {
                let path = finding["path"].as_str().unwrap_or("");
                findings.push(format!("{file}{path}"));
            }
        }
    }
    (output, findings)
}

#[test]
fn valid_expressions_pass_the_expression_syntax_check() {
    let (workflow, markers) = emit_workflow("valid");
    assert!(
        markers >= MIN_VALID_EXPRESSIONS,
        "the `valid` fixture must emit at least {MIN_VALID_EXPRESSIONS} expressions, \
         found {markers}",
    );

    let dir = TempDir::new("valid");
    write_workflow(dir.path(), &workflow);
    let (lint, findings) = lint_expression_findings(dir.path());
    assert!(
        findings.is_empty(),
        "valid expressions were rejected at {findings:?}.\n{}",
        describe(&lint),
    );
}

#[test]
fn malformed_expression_is_reported() {
    let (workflow, markers) = emit_workflow("invalid");
    assert!(
        markers >= 1,
        "the `invalid` fixture must emit at least one expression, found {markers}",
    );

    let dir = TempDir::new("invalid");
    write_workflow(dir.path(), &workflow);
    let (lint, findings) = lint_expression_findings(dir.path());
    assert!(
        !findings.is_empty(),
        "the malformed expression produced no {EXPRESSION_RULE} finding.\n{}",
        describe(&lint),
    );
}

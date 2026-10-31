//! Behavioral tests for the Python language integration: these actually
//! execute the generated Python source with `python3` and parse its stdout
//! back as JSON, so they exercise real round-tripping (quoting, unicode,
//! numeric precision, key order) rather than just inspecting generated text.
//!
//! The module under test is included directly via `#[path]` so this file
//! does not depend on the rest of the crate (`src/main.rs`, other language
//! modules, manifest handling) having landed or compiled yet; it only needs
//! `src/languages/python.rs` plus the three frozen `GeneratedFile` /
//! `GeneratedSource` / `CommandSpec` shapes, mirrored locally below to match
//! `src/languages/mod.rs` exactly.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

mod languages {
    use std::path::PathBuf;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct GeneratedFile {
        pub path: PathBuf,
        pub content: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct GeneratedSource {
        pub entrypoint: PathBuf,
        pub files: Vec<GeneratedFile>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CommandSpec {
        pub program: String,
        pub args: Vec<String>,
    }
}

#[path = "../src/languages/python.rs"]
mod python;

/// Create a fresh, unique temp directory for one test's generated source.
fn temp_dir(label: &str) -> PathBuf {
    let unique = format!(
        "loopt-python-test-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    );
    let mut dir = std::env::temp_dir();
    dir.push(unique);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Run `python::emit` + `python::command` end-to-end: write the generated
/// module to disk, execute it exactly as `loopt gen` would, and parse the
/// one JSON object it prints on stdout. Returns the parsed value and the
/// raw stdout text (the latter for order-sensitive assertions, since
/// `Value` equality ignores map ordering).
fn run_generated(workflow: &Value, stem_rel: &str, label: &str) -> (Value, String) {
    let dir = temp_dir(label);
    let stem = Path::new(stem_rel);

    let source = python::emit(workflow, stem).expect("emit should succeed");
    assert_eq!(
        source.files.len(),
        1,
        "python emit should produce exactly one file, got {:?}",
        source.files
    );
    assert_eq!(source.files[0].path, source.entrypoint);
    assert_eq!(source.entrypoint, stem.with_extension("py"));

    let file_path = dir.join(&source.entrypoint);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs for generated source");
    }
    fs::write(&file_path, &source.files[0].content).expect("write generated source");

    let cmd = python::command(&file_path).expect("command should succeed");
    assert_eq!(cmd.program, "python3");

    let output = Command::new(&cmd.program)
        .args(&cmd.args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", cmd.program));

    assert!(
        output.status.success(),
        "generated python exited with failure.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout must be valid utf-8");
    let value: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must be exactly one JSON object ({e}): {stdout}"));

    let _ = fs::remove_dir_all(&dir);
    (value, stdout)
}

#[test]
fn preserves_quote_and_backslash_characters() {
    let workflow = json!({
        "name": "both \"double\" and 'single' quotes, plus a \\backslash\\ and a\ttab",
    });
    let (value, _) = run_generated(&workflow, ".loopt/workflows/quotes", "quotes");
    assert_eq!(value, workflow);
}

#[test]
fn preserves_unicode_text_verbatim() {
    let workflow = json!({
        "name": "Déploiement 🚀 — 日本語 and Ñandú",
        "description": "emoji family: 👨‍👩‍👧‍👦",
    });
    let (value, stdout) = run_generated(&workflow, ".loopt/workflows/unicode", "unicode");
    assert_eq!(value, workflow);
    assert!(
        stdout.contains('🚀') && stdout.contains("日本語"),
        "stdout should contain literal unicode rather than escape sequences: {stdout}"
    );
}

#[test]
fn preserves_multiline_shell_scripts() {
    let workflow = json!({
        "jobs": {
            "build": {
                "steps": [
                    {
                        "run": "set -euo pipefail\necho \"line one\"\nif [ -f go.mod ]; then\n  go build ./...\nfi\n",
                    }
                ]
            }
        }
    });
    let (value, _) = run_generated(&workflow, ".loopt/workflows/multiline", "multiline");
    assert_eq!(value, workflow);
}

#[test]
fn preserves_integer_and_float_number_types_and_precision() {
    let workflow = json!({
        "small_int": 7,
        "negative_int": -42,
        "zero": 0,
        "large_int": 9_007_199_254_740_993i64,
        "whole_float": 5.0,
        "fraction": 1.2345,
        "scientific": 6.022e23,
    });
    let (value, stdout) = run_generated(&workflow, ".loopt/workflows/numbers", "numbers");
    assert_eq!(value, workflow);

    // An int-valued float (5.0) must round-trip as a float, not collapse
    // into the plain int 5.
    assert!(
        stdout.contains("\"whole_float\": 5.0") || stdout.contains("\"whole_float\":5.0"),
        "whole_float must stay a float in stdout: {stdout}"
    );
    // A large integer must not lose precision by passing through f64.
    assert!(
        stdout.contains("9007199254740993"),
        "large_int must keep full precision in stdout: {stdout}"
    );
}

#[test]
fn preserves_object_key_order_including_matrix_axes() {
    let mut matrix = serde_json::Map::new();
    matrix.insert(
        "os".to_string(),
        json!(["ubuntu-latest", "macos-latest", "windows-latest"]),
    );
    matrix.insert("node".to_string(), json!([18, 20, 22]));
    matrix.insert(
        "include".to_string(),
        json!([{"os": "ubuntu-latest", "node": 22, "experimental": true}]),
    );
    let workflow = Value::Object(matrix);

    let (value, stdout) = run_generated(&workflow, ".loopt/workflows/matrix", "matrix");
    assert_eq!(value, workflow);

    let os_pos = stdout.find("\"os\"").expect("os key present");
    let node_pos = stdout.find("\"node\"").expect("node key present");
    let include_pos = stdout.find("\"include\"").expect("include key present");
    assert!(
        os_pos < node_pos && node_pos < include_pos,
        "matrix axis key order must be preserved exactly as authored, got stdout: {stdout}"
    );
}

#[test]
fn command_runs_python3_directly_with_no_shell_involvement() {
    let source_path = Path::new("/workspace/.loopt/workflows/sample.py");
    let cmd = python::command(source_path).expect("command should succeed");
    assert_eq!(cmd.program, "python3");
    assert_eq!(
        cmd.args,
        vec!["/workspace/.loopt/workflows/sample.py".to_string()]
    );
}

#[test]
fn emit_places_entrypoint_under_stem_with_py_extension() {
    let workflow = json!({"name": "ci"});
    let stem = Path::new(".loopt/workflows/ci");
    let source = python::emit(&workflow, stem).expect("emit should succeed");

    assert_eq!(source.entrypoint, PathBuf::from(".loopt/workflows/ci.py"));
    assert_eq!(source.files.len(), 1);
    assert_eq!(source.files[0].path, source.entrypoint);
}

#[test]
fn generated_source_constructs_real_python_values_not_an_embedded_blob() {
    let workflow = json!({"on": "push", "jobs": {}});
    let source = python::emit(&workflow, Path::new(".loopt/workflows/plain")).unwrap();
    let content = &source.files[0].content;

    // The workflow is built as actual dict/list literals a user can edit,
    // not embedded as an opaque JSON/YAML string that gets parsed back.
    assert!(content.contains("def build_workflow():"));
    assert!(content.contains("'on': 'push'"));
    assert!(content.contains("'jobs': {}"));
    assert!(!content.contains("json.loads("));
    assert!(!content.contains("yaml.safe_load("));
    assert!(content.contains("json.dump(workflow, sys.stdout"));
}

//! Behavioral tests for the TypeScript language integration: these actually
//! execute the generated TypeScript source with `node` (native type
//! stripping, no build step) and parse its stdout back as JSON, so they
//! exercise real round-tripping (quoting, unicode, numeric precision, key
//! order, and the JS-specific numeric-key-reordering / `__proto__` pitfalls)
//! rather than just inspecting generated text.
//!
//! The module under test is included directly via `#[path]` so this file
//! does not depend on the rest of the crate (`src/main.rs`, other language
//! modules, manifest handling) having landed or compiled yet; it only needs
//! `src/languages/typescript.rs` plus the three frozen `GeneratedFile` /
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

#[path = "../src/languages/typescript.rs"]
mod typescript;

/// Create a fresh, unique temp directory for one test's generated source.
fn temp_dir(label: &str) -> PathBuf {
    let unique = format!(
        "loopt-typescript-test-{label}-{}-{}",
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

/// Run `typescript::emit` + `typescript::command` end-to-end: write the
/// generated module to disk, execute it exactly as `loopt gen` would, and
/// parse the one JSON object it prints on stdout. Returns the parsed value
/// and the raw stdout text (the latter for order-sensitive assertions,
/// since `Value` equality ignores map ordering).
fn run_generated(workflow: &Value, stem_rel: &str, label: &str) -> (Value, String) {
    let dir = temp_dir(label);
    let stem = Path::new(stem_rel);

    let source = typescript::emit(workflow, stem).expect("emit should succeed");
    assert_eq!(
        source.files.len(),
        1,
        "typescript emit should produce exactly one file, got {:?}",
        source.files
    );
    assert_eq!(source.files[0].path, source.entrypoint);
    assert_eq!(source.entrypoint, stem.with_extension("ts"));

    let file_path = dir.join(&source.entrypoint);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs for generated source");
    }
    fs::write(&file_path, &source.files[0].content).expect("write generated source");

    let cmd = typescript::command(&file_path).expect("command should succeed");
    assert_eq!(cmd.program, "node");

    let output = Command::new(&cmd.program)
        .args(&cmd.args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", cmd.program));

    assert!(
        output.status.success(),
        "generated typescript exited with failure.\nstdout: {}\nstderr: {}",
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
fn preserves_multiline_shell_scripts_and_expressions() {
    let workflow = json!({
        "jobs": {
            "build": {
                "steps": [
                    {
                        "run": "set -euo pipefail\necho \"line one\"\nif [ -f go.mod ]; then\n  go build ./...\nfi\n",
                    },
                    {
                        "run": "echo ${{ matrix.os }} ${{ github.sha }}",
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
        "whole_float": 5.0,
        "fraction": 1.2345,
        "scientific": 6.022e23,
    });
    let (value, stdout) = run_generated(&workflow, ".loopt/workflows/numbers", "numbers");
    assert_eq!(value, workflow);

    // An int-valued float (5.0) must round-trip as a float, not collapse
    // into the plain int 5.
    assert!(
        stdout.contains("\"whole_float\":5.0") || stdout.contains("\"whole_float\": 5.0"),
        "whole_float must stay a float in stdout: {stdout}"
    );
}

#[test]
fn preserves_huge_integer_precision_beyond_js_safe_integer_range() {
    // 2^63 - 1: far outside Number.MAX_SAFE_INTEGER (2^53 - 1). A naive
    // `JSON.stringify`/`Number` round trip would silently corrupt this.
    let workflow = json!({
        "run_id": 9_223_372_036_854_775_807i64,
        "small": 42,
    });
    let (value, stdout) = run_generated(&workflow, ".loopt/workflows/huge-int", "huge-int");
    assert_eq!(value, workflow);
    assert!(
        stdout.contains("9223372036854775807"),
        "huge integer must keep full precision in stdout: {stdout}"
    );

    let content = typescript::emit(&workflow, Path::new(".loopt/workflows/huge-int"))
        .unwrap()
        .files
        .remove(0)
        .content;
    assert!(
        content.contains("raw(\"9223372036854775807\")"),
        "huge integers must be emitted via the raw() exact-text helper, got: {content}"
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

/// JS object literals reorder own keys that look like array indices ("0",
/// "1", ...) ahead of every other key, no matter the insertion order. This
/// is the one ordering pitfall Python/Go dicts do not have, and it is the
/// reason `typescript.rs` routes such objects through `ordered([...])`.
#[test]
fn preserves_purely_numeric_key_order_against_js_array_index_reordering() {
    let mut obj = serde_json::Map::new();
    // Deliberately out of ascending order, and with a non-numeric key in
    // the middle: a plain JS object literal would reorder this to
    // "0", "1", "2", "zz" regardless of what we write below.
    obj.insert("2".to_string(), json!("second"));
    obj.insert("zz".to_string(), json!("last-ish"));
    obj.insert("0".to_string(), json!("zeroth"));
    obj.insert("1".to_string(), json!("first"));
    let workflow = Value::Object(obj);

    let (value, stdout) = run_generated(&workflow, ".loopt/workflows/numeric-keys", "numeric-keys");
    assert_eq!(value, workflow);

    let pos = |needle: &str| {
        stdout
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} present in {stdout}"))
    };
    let p2 = pos("\"2\"");
    let pzz = pos("\"zz\"");
    let p0 = pos("\"0\"");
    let p1 = pos("\"1\"");
    assert!(
        p2 < pzz && pzz < p0 && p0 < p1,
        "numeric-looking key order must be preserved exactly as authored (2, zz, 0, 1), got stdout: {stdout}"
    );

    let content = typescript::emit(&workflow, Path::new(".loopt/workflows/numeric-keys"))
        .unwrap()
        .files
        .remove(0)
        .content;
    assert!(
        content.contains("ordered(["),
        "objects with numeric-looking keys must go through the ordered() helper, got: {content}"
    );
}

/// `__proto__` used as a plain (non-computed) object literal key reassigns
/// the object's prototype instead of creating an own property -- a classic
/// prototype-pollution footgun. The generator must emit it safely.
#[test]
fn emits_proto_named_keys_as_safe_data_properties() {
    let workflow = json!({
        "jobs": {
            "build": {
                "steps": [
                    {
                        "__proto__": "literal-string-value-not-a-prototype",
                        "name": "Run",
                    }
                ]
            }
        }
    });
    let (value, stdout) = run_generated(&workflow, ".loopt/workflows/proto-key", "proto-key");
    // If `__proto__` had instead reassigned the prototype rather than
    // creating an own property, it would not show up as an enumerable own
    // key and `JSON.stringify`-equivalent output would be missing it
    // entirely, breaking this equality.
    assert_eq!(value, workflow);
    assert!(stdout.contains("\"__proto__\":\"literal-string-value-not-a-prototype\""));

    let content = typescript::emit(&workflow, Path::new(".loopt/workflows/proto-key"))
        .unwrap()
        .files
        .remove(0)
        .content;
    assert!(
        content.contains("[\"__proto__\"]:"),
        "__proto__ keys must use computed-property syntax, got: {content}"
    );
}

#[test]
fn command_runs_node_directly_with_no_shell_involvement() {
    let source_path = Path::new("/workspace/.loopt/workflows/sample.ts");
    let cmd = typescript::command(source_path).expect("command should succeed");
    assert_eq!(cmd.program, "node");
    assert_eq!(
        cmd.args,
        vec!["/workspace/.loopt/workflows/sample.ts".to_string()]
    );
}

#[test]
fn emit_places_entrypoint_under_stem_with_ts_extension() {
    let workflow = json!({"name": "ci"});
    let stem = Path::new(".loopt/workflows/ci");
    let source = typescript::emit(&workflow, stem).expect("emit should succeed");

    assert_eq!(source.entrypoint, PathBuf::from(".loopt/workflows/ci.ts"));
    assert_eq!(source.files.len(), 1);
    assert_eq!(source.files[0].path, source.entrypoint);
}

#[test]
fn generated_source_constructs_real_typescript_values_not_an_embedded_blob() {
    let workflow = json!({"on": "push", "jobs": {}});
    let source = typescript::emit(&workflow, Path::new(".loopt/workflows/plain")).unwrap();
    let content = &source.files[0].content;

    // The workflow is built as actual object/array literals a user can
    // edit, not embedded as an opaque JSON/YAML string that gets parsed
    // back at runtime.
    assert!(content.contains("const workflow: JsonValue = {"));
    assert!(content.contains("on: \"push\""));
    assert!(content.contains("jobs: {}"));
    assert!(!content.contains("JSON.parse("));
    assert!(!content.contains("yaml.load("));
    assert!(content.contains("process.stdout.write(stringify(workflow)"));
}

#[test]
fn generated_source_runs_with_plain_node_and_no_extra_flags() {
    // Regression guard: Node's TypeScript stripping is unflagged by
    // default in the supported toolchain version, and `command()` must not
    // depend on a flag that may not exist elsewhere.
    let workflow = json!({"name": "ci"});
    let stem = Path::new(".loopt/workflows/flagless");
    let source = typescript::emit(&workflow, stem).unwrap();
    let cmd = typescript::command(&source.entrypoint).unwrap();
    assert_eq!(
        cmd.args.len(),
        1,
        "command must pass only the source path, no flags: {cmd:?}"
    );
}

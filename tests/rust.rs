//! Integration tests for the Rust language integration
//! (`src/languages/rust.rs`): nested objects/arrays, string escaping,
//! number formatting, and the generated `<stem>/Cargo.toml` /
//! `<stem>/main.rs` / `cargo run` command wiring.
//!
//! These tests inspect the generated Rust/TOML source as text and the
//! structure of `GeneratedSource`/`CommandSpec`; they do not shell out to
//! `cargo` (no builds/tests are run by this suite -- see assignment notes).

use std::path::{Path, PathBuf};

use loopt::languages::rust;
use serde_json::json;

fn file_content<'a>(source: &'a loopt::languages::GeneratedSource, path: &Path) -> &'a str {
    source
        .files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("no generated file at {}", path.display()))
        .content
        .as_str()
}

#[test]
fn entrypoint_and_manifest_paths_are_project_relative_under_stem() {
    let stem = PathBuf::from(".loopt/workflows/ci");
    let source = rust::emit(&json!({"name": "CI"}), &stem).expect("emit should succeed");

    assert_eq!(source.entrypoint, stem.join("main.rs"));

    let paths: Vec<&PathBuf> = source.files.iter().map(|f| &f.path).collect();
    assert!(paths.contains(&&stem.join("main.rs")));
    assert!(paths.contains(&&stem.join("Cargo.toml")));
    assert_eq!(source.files.len(), 2);
}

#[test]
fn manifest_is_a_standalone_workspace_with_matching_bin_and_required_features() {
    let stem = PathBuf::from(".loopt/workflows/deploy");
    let source = rust::emit(&json!({"on": "push"}), &stem).expect("emit should succeed");

    let manifest = file_content(&source, &stem.join("Cargo.toml"));

    // Standalone workspace: never folded into an ancestor workspace.
    assert!(
        manifest.contains("[workspace]"),
        "manifest missing [workspace]:\n{manifest}"
    );
    assert!(
        !manifest.contains("members"),
        "empty [workspace] must not list members:\n{manifest}"
    );

    // Package/bin names match (derived from the stem's final component).
    assert!(
        manifest.contains("name = \"deploy\""),
        "manifest:\n{manifest}"
    );
    assert!(manifest.contains("[[bin]]"), "manifest:\n{manifest}");
    assert!(
        manifest.contains("path = \"main.rs\""),
        "manifest:\n{manifest}"
    );

    // serde_json needs preserve_order (key order) and arbitrary_precision
    // (exact numeric round-tripping) for the generated source to be correct.
    assert!(manifest.contains("serde_json"), "manifest:\n{manifest}");
    assert!(manifest.contains("preserve_order"), "manifest:\n{manifest}");
    assert!(
        manifest.contains("arbitrary_precision"),
        "manifest:\n{manifest}"
    );
}

#[test]
fn crate_name_is_sanitized_from_non_identifier_stem_components() {
    let stem = PathBuf::from(".loopt/workflows/My Workflow! 2.yml");
    let source = rust::emit(&json!({}), &stem).expect("emit should succeed");
    let manifest = file_content(&source, &stem.join("Cargo.toml"));

    // Sanitized: lowercase, non-alphanumerics -> '_'.
    assert!(
        manifest.contains("name = \"my_workflow__2_yml\""),
        "manifest:\n{manifest}"
    );
}

#[test]
fn crate_name_gets_prefixed_when_it_would_not_start_with_a_letter() {
    let stem = PathBuf::from(".loopt/workflows/123");
    let source = rust::emit(&json!({}), &stem).expect("emit should succeed");
    let manifest = file_content(&source, &stem.join("Cargo.toml"));

    assert!(
        manifest.contains("name = \"wf_123\""),
        "manifest:\n{manifest}"
    );
}

#[test]
fn nested_objects_and_arrays_preserve_structure_and_key_order() {
    let stem = PathBuf::from(".loopt/workflows/matrix");
    let workflow = json!({
        "zebra": 1,
        "apple": {
            "jobs": {
                "build": {
                    "strategy": {
                        "matrix": {
                            "os": ["ubuntu-latest", "macos-latest"],
                            "node": [20, 22]
                        }
                    }
                }
            }
        }
    });
    let source = rust::emit(&workflow, &stem).expect("emit should succeed");
    let main_rs = file_content(&source, &stem.join("main.rs"));

    // Structural helpers used throughout.
    assert!(main_rs.contains("fn build_workflow() -> Value {"));
    assert!(main_rs.contains("fn obj(entries: Vec<(&str, Value)>) -> Value {"));
    assert!(main_rs.contains("fn arr(items: Vec<Value>) -> Value {"));
    assert!(main_rs.contains("fn num(repr: &str) -> Value {"));
    assert!(main_rs.contains("fn main() {"));
    assert!(main_rs.contains("println!(\"{rendered}\");"));

    // Top-level key order preserved: "zebra" authored before "apple".
    let zebra_pos = main_rs.find("\"zebra\"").expect("zebra key present");
    let apple_pos = main_rs.find("\"apple\"").expect("apple key present");
    assert!(zebra_pos < apple_pos, "key order not preserved:\n{main_rs}");

    // Matrix axis order preserved within their array (not sorted).
    let ubuntu_pos = main_rs.find("ubuntu-latest").expect("ubuntu entry present");
    let macos_pos = main_rs.find("macos-latest").expect("macos entry present");
    assert!(
        ubuntu_pos < macos_pos,
        "matrix os axis order not preserved:\n{main_rs}"
    );

    let node20_pos = main_rs.find("num(\"20\")").expect("node 20 entry present");
    let node22_pos = main_rs.find("num(\"22\")").expect("node 22 entry present");
    assert!(
        node20_pos < node22_pos,
        "matrix node axis order not preserved:\n{main_rs}"
    );

    // Nesting uses obj()/arr() recursively, not an opaque embedded string.
    assert!(main_rs.contains("\"matrix\""));
    assert!(!main_rs.contains("serde_json::from_str::<Value>"));
}

#[test]
fn strings_are_escaped_as_valid_rust_literals() {
    let stem = PathBuf::from(".loopt/workflows/strings");
    let workflow = json!({
        "quote": "she said \"hi\"",
        "backslash": "C:\\path\\to\\file",
        "newline": "line one\nline two",
        "tab": "a\tb",
        "control": "\u{0007}bell",
        "unicode": "caf\u{e9} \u{1F600}",
    });
    let source = rust::emit(&workflow, &stem).expect("emit should succeed");
    let main_rs = file_content(&source, &stem.join("main.rs"));

    assert!(
        main_rs.contains(r#""she said \"hi\"""#),
        "quote escaping:\n{main_rs}"
    );
    assert!(
        main_rs.contains(r#""C:\\path\\to\\file""#),
        "backslash escaping:\n{main_rs}"
    );
    assert!(
        main_rs.contains(r#""line one\nline two""#),
        "newline escaping:\n{main_rs}"
    );
    assert!(main_rs.contains(r#""a\tb""#), "tab escaping:\n{main_rs}");
    assert!(
        main_rs.contains(r#""\u{7}bell""#),
        "control-char escaping:\n{main_rs}"
    );
    // Printable Unicode is emitted verbatim (valid in UTF-8 Rust source).
    assert!(
        main_rs.contains("caf\u{e9} \u{1F600}"),
        "unicode should be emitted verbatim:\n{main_rs}"
    );
}

#[test]
fn numbers_round_trip_exact_textual_representation() {
    let stem = PathBuf::from(".loopt/workflows/numbers");
    let workflow = json!({
        "int": 42,
        "negative": -7,
        "zero": 0,
        "float": 3.5,
        "small_float": 0.1,
    });
    let source = rust::emit(&workflow, &stem).expect("emit should succeed");
    let main_rs = file_content(&source, &stem.join("main.rs"));

    assert!(main_rs.contains("num(\"42\")"), "int:\n{main_rs}");
    assert!(main_rs.contains("num(\"-7\")"), "negative:\n{main_rs}");
    assert!(main_rs.contains("num(\"0\")"), "zero:\n{main_rs}");
    assert!(main_rs.contains("num(\"3.5\")"), "float:\n{main_rs}");
    assert!(main_rs.contains("num(\"0.1\")"), "small_float:\n{main_rs}");

    // Numbers are built via Number::from_str on the exact source text, not
    // coerced through an f64/i64 literal the Rust compiler could re-round.
    assert!(main_rs.contains("Number::from_str(repr)"));
}

#[test]
fn empty_objects_and_arrays_render_without_trailing_commas_or_panics() {
    let stem = PathBuf::from(".loopt/workflows/empty");
    let workflow = json!({"jobs": {}, "steps": []});
    let source = rust::emit(&workflow, &stem).expect("emit should succeed");
    let main_rs = file_content(&source, &stem.join("main.rs"));

    assert!(main_rs.contains("obj(vec![])"), "empty object:\n{main_rs}");
    assert!(main_rs.contains("arr(vec![])"), "empty array:\n{main_rs}");
}

#[test]
fn bool_and_null_scalars() {
    let stem = PathBuf::from(".loopt/workflows/scalars");
    let workflow = json!({"continue_on_error": true, "fail_fast": false, "timeout": null});
    let source = rust::emit(&workflow, &stem).expect("emit should succeed");
    let main_rs = file_content(&source, &stem.join("main.rs"));

    assert!(main_rs.contains("Value::Bool(true)"));
    assert!(main_rs.contains("Value::Bool(false)"));
    assert!(main_rs.contains("Value::Null"));
}

#[test]
fn command_runs_cargo_with_project_manifest_and_scoped_target_dir() {
    let main_rs_path = PathBuf::from(".loopt/workflows/ci/deploy/main.rs");
    let cmd = rust::command(&main_rs_path).expect("command should succeed");

    assert_eq!(cmd.program, "cargo");
    assert_eq!(cmd.args[0], "run");

    let manifest_idx = cmd
        .args
        .iter()
        .position(|a| a == "--manifest-path")
        .expect("--manifest-path flag present");
    assert_eq!(
        cmd.args[manifest_idx + 1],
        ".loopt/workflows/ci/deploy/Cargo.toml"
    );

    let target_idx = cmd
        .args
        .iter()
        .position(|a| a == "--target-dir")
        .expect("--target-dir flag present");
    // Shared repository cache under .loopt/.cache, never inside
    // .loopt/workflows (so build artifacts never sit next to human-edited
    // source) and never a per-project cold build.
    assert_eq!(cmd.args[target_idx + 1], ".loopt/.cache/rust-target");

    let bin_idx = cmd
        .args
        .iter()
        .position(|a| a == "--bin")
        .expect("--bin flag present");
    assert_eq!(cmd.args[bin_idx + 1], "deploy");

    assert!(cmd.args.iter().any(|a| a == "--quiet"));

    // No system-wide cargo path assumption: the bare "cargo" program name
    // resolves via whatever PATH the caller configures.
    assert!(!cmd.program.contains('/'));
}

#[test]
fn command_scopes_target_dir_even_outside_loopt_workflows() {
    // Defensive: if ever invoked on a source path that isn't under
    // .loopt/workflows, the cache dir still lands under .loopt/.cache
    // rather than panicking or escaping the repo root.
    let main_rs_path = PathBuf::from("custom/place/main.rs");
    let cmd = rust::command(&main_rs_path).expect("command should succeed");

    let target_idx = cmd
        .args
        .iter()
        .position(|a| a == "--target-dir")
        .expect("--target-dir flag present");
    assert_eq!(cmd.args[target_idx + 1], ".loopt/.cache/rust-target");
}

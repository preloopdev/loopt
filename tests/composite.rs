//! Real CLI round trips for repository-local composite action metadata.
//!
//! These tests deliberately use the same generated-language path as users do:
//! `loopt import` emits a native source file, and `loopt gen` executes that
//! source again. No action document is embedded as an opaque fixture in the
//! test process.

mod support;

use std::fs;

use serde_json::Value;
use support::{assert_order_preserving_eq, parse_document, TestRepo};

const ROOT_ACTION: &str = r#"name: composite root
description: Keeps metadata and shell choices intact.
inputs:
  greeting:
    description: Greeting text
    required: true
    default: hello
    x-input-extension: keep-me
outputs:
  result:
    description: The generated result
    value: ${{ steps.make.outputs.result }}
    x-output-extension: keep-me
x-action-extension:
  owner: test
runs:
  using: composite
  steps:
    - id: make
      name: Make result
      shell: bash
      run: echo first >> "$GITHUB_OUTPUT"
    - name: Check result
      shell: python
      run: echo second
"#;

const NESTED_ACTION: &str = r#"name: nested composite
description: Nested action metadata.
x-custom: preserved
runs:
  using: composite
  steps:
    - name: nested
      shell: sh
      run: echo nested
"#;

const WORKFLOW: &str = r#"name: workflow called action
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: echo workflow
"#;

fn manifest(repo: &TestRepo) -> Value {
    serde_json::from_str(&repo.read(".loopt/manifest.json")).expect("manifest JSON")
}

#[test]
fn root_action_round_trips_preserving_metadata_and_supports_check() {
    let repo = TestRepo::new("composite-root");
    repo.write("action.yml", ROOT_ACTION);

    repo.run(&["import", "action.yml", "--language", "python"])
        .assert_success();
    let doc = manifest(&repo);
    assert_eq!(doc["entries"][0]["document_kind"], "action");
    assert_eq!(
        doc["entries"][0]["source"],
        ".loopt/workflows/actions/root/action.py"
    );

    // No source edit means action metadata is byte-stable, including comments
    // and the user's chosen action.yml spelling.
    repo.run(&["gen", "--no-lint"]).assert_success();
    assert_eq!(repo.read("action.yml"), ROOT_ACTION);
    let original = parse_document(&repo.path("action.yml")).expect("root action parses");
    assert_eq!(original.value()["runs"]["using"], "composite");
    assert_eq!(original.value()["runs"]["steps"][0]["shell"], "bash");
    assert_eq!(
        original.value()["inputs"]["greeting"]["x-input-extension"],
        "keep-me"
    );
    assert_eq!(
        original.value()["outputs"]["result"]["x-output-extension"],
        "keep-me"
    );
    assert_eq!(original.value()["x-action-extension"]["owner"], "test");

    let manifest_after = manifest(&repo);
    let source = manifest_after["entries"][0]["source"]
        .as_str()
        .expect("source path")
        .to_owned();
    let edited = repo.read(&source).replace("echo first", "echo changed");
    repo.write(&source, &edited);

    let check = repo.run(&["gen", "--check", "--no-lint"]);
    check.assert_failure();
    assert!(
        check.stdout.contains("stale action.yml"),
        "check should report action drift: {}",
        check.stdout
    );
    assert_eq!(repo.read("action.yml"), ROOT_ACTION);

    repo.run(&["gen", "--no-lint"]).assert_success();
    let generated = parse_document(&repo.path("action.yml")).expect("generated action parses");
    assert_eq!(generated.value()["runs"]["using"], "composite");
    assert_eq!(generated.value()["runs"]["steps"][0]["shell"], "bash");
    assert_eq!(
        generated.value()["runs"]["steps"][0]["run"],
        "echo changed >> \"$GITHUB_OUTPUT\""
    );
    assert_eq!(
        generated.value()["inputs"]["greeting"]["x-input-extension"],
        "keep-me"
    );
    assert_eq!(
        generated.value()["outputs"]["result"]["x-output-extension"],
        "keep-me"
    );
    assert_eq!(generated.value()["x-action-extension"]["owner"], "test");
}

#[test]
fn nested_action_yaml_round_trips_to_original_path() {
    let repo = TestRepo::new("composite-nested");
    let action_path = ".github/actions/example/action.yaml";
    repo.write(action_path, NESTED_ACTION);

    repo.run(&["import", action_path, "--language", "python"])
        .assert_success();
    let manifest = manifest(&repo);
    assert_eq!(manifest["entries"][0]["document_kind"], "action");
    assert_eq!(manifest["entries"][0]["output"], action_path);
    assert_eq!(
        manifest["entries"][0]["source"],
        ".loopt/workflows/actions/example/action-yaml.py"
    );

    // Default generation must discover the managed action without requiring
    // the source path a second time.
    repo.run(&["gen", "--no-lint"]).assert_success();
    assert_eq!(repo.read(action_path), NESTED_ACTION);
}

#[test]
fn workflow_and_action_filename_collisions_are_isolated() {
    let repo = TestRepo::new("composite-collisions");
    repo.write("action.yml", ROOT_ACTION);
    repo.write(".github/actions/example/action.yml", NESTED_ACTION);
    repo.write(".github/workflows/action.yml", WORKFLOW);

    repo.run(&[
        "import",
        "action.yml",
        ".github/actions/example/action.yml",
        ".github/workflows/action.yml",
        "--language",
        "python",
    ])
    .assert_success();

    let manifest = manifest(&repo);
    let entries = manifest["entries"].as_array().expect("manifest entries");
    assert_eq!(entries.len(), 3);
    let sources: Vec<&str> = entries
        .iter()
        .map(|entry| entry["source"].as_str().expect("source"))
        .collect();
    assert_eq!(
        sources,
        vec![
            ".loopt/workflows/actions/example/action.py",
            ".loopt/workflows/action.py",
            ".loopt/workflows/actions/root/action.py",
        ]
    );

    repo.run(&["gen", "--no-lint"]).assert_success();
    assert_eq!(repo.read("action.yml"), ROOT_ACTION);
    assert_eq!(
        repo.read(".github/actions/example/action.yml"),
        NESTED_ACTION
    );
    assert_eq!(
        parse_document(&repo.path(".github/workflows/action.yml"))
            .expect("workflow parses")
            .value()["jobs"]["build"]["steps"][0]["run"],
        "echo workflow"
    );
}

#[test]
fn old_v1_manifest_without_document_kind_still_generates_workflow() {
    let repo = TestRepo::new("composite-old-manifest");
    let source = r#"import json
print(json.dumps({"name": "legacy", "on": "push", "jobs": {"build": {"runs-on": "ubuntu-latest", "steps": []}}}))
"#;
    repo.write(".loopt/workflows/legacy.py", source);
    repo.write(
        ".loopt/manifest.json",
        r#"{
  "version": 1,
  "entries": [{
    "language": "python",
    "source": ".loopt/workflows/legacy.py",
    "output": ".github/workflows/legacy.yml",
    "last_accepted_yaml": ""
  }]
}
"#,
    );

    repo.run(&["gen", "--no-lint"]).assert_success();
    let manifest = manifest(&repo);
    assert_eq!(manifest["entries"][0]["document_kind"], "workflow");
    assert!(repo.exists(".github/workflows/legacy.yml"));
}

#[cfg(unix)]
#[test]
fn action_import_and_generation_reject_symlink_destinations() {
    let repo = TestRepo::new("composite-security");
    let outside = TestRepo::new("composite-security-outside");
    let valuable = outside.write("valuable.yml", "do not overwrite\n");
    let valuable_action = outside.write("valuable-action.yml", ROOT_ACTION);

    repo.symlink("action.yml", &valuable);
    repo.run(&["import", "action.yml", "--language", "python"])
        .assert_failure();
    assert_eq!(outside.read("valuable.yml"), "do not overwrite\n");
    assert!(!repo.exists(".loopt/manifest.json"));

    // Import a real action first, then replace only its output with a symlink.
    repo.path("action.yml")
        .symlink_metadata()
        .expect("action symlink remains");
    fs::remove_file(repo.path("action.yml")).expect("remove action symlink");
    repo.write("action.yml", ROOT_ACTION);
    repo.run(&["import", "action.yml", "--language", "python"])
        .assert_success();
    fs::remove_file(repo.path("action.yml")).expect("remove action");
    repo.symlink("action.yml", &valuable_action);
    let manifest = manifest(&repo);
    let source = manifest["entries"][0]["source"]
        .as_str()
        .expect("source path")
        .to_owned();
    repo.write(
        &source,
        &repo.read(&source).replace("echo first", "echo changed"),
    );

    let result = repo.run(&["gen", "--no-lint"]);
    result.assert_failure();
    assert_eq!(outside.read("valuable-action.yml"), ROOT_ACTION);
}

#[test]
fn action_round_trip_keeps_nested_key_order() {
    let repo = TestRepo::new("composite-order");
    repo.write("action.yml", ROOT_ACTION);
    let expected = parse_document(&repo.path("action.yml")).expect("action parses");
    repo.run(&["import", "action.yml", "--language", "python"])
        .assert_success();
    repo.run(&["gen", "--no-lint"]).assert_success();
    let actual = parse_document(&repo.path("action.yml")).expect("generated action parses");
    assert_order_preserving_eq(expected.value(), actual.value());
}

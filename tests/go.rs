//! Behavioral tests for the Go language integration (`src/languages/go.rs`).
//!
//! Each test drives the real pipeline: `go::emit` builds a Go project and
//! its vendored SDK from a `serde_json::Value` workflow, every generated
//! file is written to a scratch directory, `go::command` is used to invoke
//! `go -C <project> run .` exactly as the CLI would, and the process's
//! stdout is checked both as raw JSON text (to assert on byte order,
//! independent of how this test's own JSON values are represented) and as a
//! parsed `serde_json::Value` (to assert on content). Requires a `go`
//! toolchain on `PATH`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use loopt::languages::{go, GeneratedSource, Language};
use serde_json::{Map, Number, Value};

/// Minimal scratch-directory guard: creates a unique temp directory and
/// removes it on drop. Avoids pulling in a dev-dependency for something
/// `std` already provides.
struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "loopt-go-test-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create scratch dir");
        ScratchDir { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn file_content<'a>(source: &'a GeneratedSource, path: &Path) -> &'a str {
    source
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("no generated file at {}", path.display()))
        .content
        .as_str()
}

/// Writes every file from `generated` under `root`, creating parents.
fn write_project(root: &Path, generated: &GeneratedSource) {
    for file in &generated.files {
        let destination = root.join(&file.path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).expect("create generated file parent dirs");
        }
        fs::write(&destination, &file.content).expect("write generated file");
    }
}

/// Runs the full emit -> write -> `go run` pipeline for `workflow` and
/// returns raw stdout text alongside the value parsed from it.
fn run_workflow(workflow: &Value, stem: &str) -> (String, Value) {
    let scratch = ScratchDir::new(&stem.replace(['/', '.'], "_"));

    let generated = go::emit(workflow, Path::new(stem)).expect("emit should succeed");
    assert_eq!(
        generated.entrypoint,
        PathBuf::from(format!("{stem}/main.go")),
        "entrypoint must be <stem>/main.go"
    );
    write_project(scratch.path(), &generated);

    let spec = go::command(&generated.entrypoint).expect("command should succeed");
    assert_eq!(spec.program, "go");
    assert_eq!(
        spec.args,
        vec!["-C", stem, "run", "."],
        "command must run the project directory"
    );

    let output = Command::new(&spec.program)
        .args(&spec.args)
        .current_dir(scratch.path())
        .output()
        .expect("go run should execute (requires `go` on PATH)");

    assert!(
        output.status.success(),
        "go run failed:\n--- generated main.go ---\n{}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        file_content(&generated, &generated.entrypoint),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout must be utf-8");
    let value: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("stdout must be one JSON object ({err}): {stdout}"));
    (stdout, value)
}

fn object(fields: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in fields {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

#[test]
fn emit_writes_a_go_project_and_vendors_the_sdk() {
    let stem = PathBuf::from(".loopt/workflows/ci");
    let generated = go::emit(&object(vec![("name", Value::String("CI".into()))]), &stem)
        .expect("emit should succeed");

    assert_eq!(generated.entrypoint, stem.join("main.go"));

    let paths: Vec<&PathBuf> = generated.files.iter().map(|file| &file.path).collect();
    assert!(paths.contains(&&stem.join("main.go")));
    assert!(paths.contains(&&stem.join("go.mod")));

    // The SDK the project's go.mod requires is written once under the
    // fixed, project-independent `.loopt/sdk/go`.
    for relative in [
        ".loopt/sdk/go/go.mod",
        ".loopt/sdk/go/om/om.go",
        ".loopt/sdk/go/expr/expr.go",
        ".loopt/sdk/go/expr/contexts.go",
        ".loopt/sdk/go/core/step.go",
        ".loopt/sdk/go/core/job.go",
        ".loopt/sdk/go/core/workflow.go",
        ".loopt/sdk/go/emit/emit.go",
    ] {
        assert!(
            paths.contains(&&PathBuf::from(relative)),
            "missing vendored SDK file {relative}"
        );
    }
    assert!(paths
        .iter()
        .all(|path| path.starts_with(".loopt/workflows") || path.starts_with(".loopt/sdk")));

    // 2 project files (go.mod, main.go) + the vendored SDK module's own
    // files; update this count alongside `go::SDK_FILES`.
    assert_eq!(generated.files.len(), 2 + 16);
}

#[test]
fn main_go_builds_native_values_through_the_sdk() {
    let stem = PathBuf::from(".loopt/workflows/build");
    let workflow = object(vec![
        ("name", Value::String("CI".into())),
        (
            "jobs",
            object(vec![(
                "build",
                object(vec![(
                    "steps",
                    Value::Array(vec![object(vec![(
                        "run",
                        Value::String("go test ./...".into()),
                    )])]),
                )]),
            )]),
        ),
    ]);
    let generated = go::emit(&workflow, &stem).expect("emit should succeed");
    let main_go = file_content(&generated, &stem.join("main.go"));

    // The project imports the SDK rather than carrying a private copy of
    // the ordered-object helper.
    assert!(main_go.contains("package main"));
    assert!(main_go.contains("\"github.com/preloopdev/loopt/sdk/go/emit\""));
    assert!(main_go.contains("\"github.com/preloopdev/loopt/sdk/go/om\""));
    assert!(main_go.contains("func workflow() any {"));
    assert!(main_go.contains("om.New("));
    assert!(main_go.contains("om.F("));
    assert!(main_go.contains("emit.Print(workflow())"));

    // No inline runtime: the helper lives in the SDK now.
    assert!(!main_go.contains("type om struct"));
    assert!(!main_go.contains("func newOM"));
    assert!(!main_go.contains("type omField struct"));
    assert!(!main_go.contains("func f("));

    // Values are real Go values, not an embedded JSON string.
    assert!(main_go.contains("go test ./..."));
    assert!(!main_go.contains("json.Unmarshal"));
    assert!(!main_go.contains("json.NewDecoder"));
}

#[test]
fn manifest_requires_the_vendored_sdk_with_a_relative_replace() {
    let stem = PathBuf::from(".loopt/workflows/deploy");
    let generated = go::emit(&object(vec![("on", Value::String("push".into()))]), &stem)
        .expect("emit should succeed");
    let manifest = file_content(&generated, &stem.join("go.mod"));

    assert!(
        manifest.contains("module loopt.local/workflows/deploy"),
        "{manifest}"
    );
    assert!(manifest.contains("go 1.21"), "{manifest}");
    assert!(
        manifest.contains("require github.com/preloopdev/loopt/sdk/go v0.0.0"),
        "{manifest}"
    );
    assert!(
        manifest.contains("replace github.com/preloopdev/loopt/sdk/go => ../../sdk/go"),
        "{manifest}"
    );
}

#[test]
fn nested_project_stems_get_a_correct_relative_sdk_path() {
    let stem = PathBuf::from(".loopt/workflows/actions/deploy/ci");
    let generated = go::emit(&object(vec![]), &stem).expect("emit should succeed");
    let manifest = file_content(&generated, &stem.join("go.mod"));

    assert!(
        manifest.contains("replace github.com/preloopdev/loopt/sdk/go => ../../../../sdk/go"),
        "{manifest}"
    );
    assert!(
        manifest.contains("module loopt.local/workflows/ci"),
        "{manifest}"
    );
}

#[test]
fn module_name_is_sanitized_from_non_identifier_stem_components() {
    let stem = PathBuf::from(".loopt/workflows/My Workflow! 2.yml");
    let generated = go::emit(&object(vec![]), &stem).expect("emit should succeed");
    let manifest = file_content(&generated, &stem.join("go.mod"));

    assert!(
        manifest.contains("module loopt.local/workflows/my_workflow__2_yml"),
        "{manifest}"
    );
}

#[test]
fn module_name_gets_prefixed_when_it_would_not_start_with_a_letter() {
    let stem = PathBuf::from(".loopt/workflows/123");
    let generated = go::emit(&object(vec![]), &stem).expect("emit should succeed");
    let manifest = file_content(&generated, &stem.join("go.mod"));

    assert!(
        manifest.contains("module loopt.local/workflows/wf_123"),
        "{manifest}"
    );
}

/// Deliberately non-alphabetical key order (zebra before apple, zzz before
/// aaa) at both the top level and inside a nested `strategy.matrix` object,
/// mirroring how GitHub Actions matrix axes must stay in author order.
/// Native `map[string]any` + `encoding/json` in Go would sort these
/// alphabetically; the SDK's ordered object must not.
#[test]
fn nested_mapping_order_is_preserved() {
    let workflow = object(vec![
        ("zebra-key", Value::String("first".into())),
        ("apple-key", Value::String("second".into())),
        (
            "jobs",
            object(vec![(
                "build",
                object(vec![
                    ("runs-on", Value::String("ubuntu-latest".into())),
                    (
                        "strategy",
                        object(vec![(
                            "matrix",
                            object(vec![
                                (
                                    "os",
                                    Value::Array(vec![
                                        Value::String("macos-latest".into()),
                                        Value::String("ubuntu-latest".into()),
                                    ]),
                                ),
                                (
                                    "node",
                                    Value::Array(vec![
                                        Value::Number(Number::from(20)),
                                        Value::Number(Number::from(18)),
                                        Value::Number(Number::from(16)),
                                    ]),
                                ),
                            ]),
                        )]),
                    ),
                ]),
            )]),
        ),
    ]);

    let (stdout, parsed) = run_workflow(&workflow, ".loopt/workflows/order");

    // Raw-text order check: independent of whether this test binary's own
    // serde_json::Map happens to preserve insertion order, this asserts on
    // the actual bytes `go run` printed.
    let zebra_pos = stdout.find("\"zebra-key\"").expect("zebra-key present");
    let apple_pos = stdout.find("\"apple-key\"").expect("apple-key present");
    assert!(
        zebra_pos < apple_pos,
        "top-level keys must keep author order (zebra-key before apple-key): {stdout}"
    );

    let os_pos = stdout.find("\"os\"").expect("matrix os key present");
    let node_pos = stdout.find("\"node\"").expect("matrix node key present");
    assert!(
        os_pos < node_pos,
        "matrix axis keys must keep author order (os before node): {stdout}"
    );

    // Matrix axis element order (array order, not an ordering concern for
    // `om`, but still must round-trip exactly: node is [20, 18, 16]).
    let node_values: Vec<i64> = parsed["jobs"]["build"]["strategy"]["matrix"]["node"]
        .as_array()
        .expect("node matrix axis is an array")
        .iter()
        .map(|v| v.as_i64().expect("node entries are integers"))
        .collect();
    assert_eq!(node_values, vec![20, 18, 16]);
}

/// Strings containing quotes, backslashes, control characters, a literal
/// backtick (forcing the interpreted-literal fallback instead of a raw
/// Go string), and multi-byte Unicode must all round-trip exactly.
#[test]
fn strings_round_trip_through_escaping() {
    let shell_script = "echo \"hi\"\nif [ -n \"$X\" ]; then\n\techo done\nfi\n";
    let backtick_value = "uses `cmd` substitution and a \\backslash\\";
    let unicode_value = "héllo wörld — 日本語 🎉\ttabbed";
    let control_value = "bell\u{7}unit-sep\u{1f}end";

    let workflow = object(vec![(
        "jobs",
        object(vec![(
            "build",
            object(vec![(
                "steps",
                Value::Array(vec![
                    object(vec![("run", Value::String(shell_script.to_string()))]),
                    object(vec![("name", Value::String(backtick_value.to_string()))]),
                    object(vec![("name", Value::String(unicode_value.to_string()))]),
                    object(vec![("name", Value::String(control_value.to_string()))]),
                ]),
            )]),
        )]),
    )]);

    let (_stdout, parsed) = run_workflow(&workflow, ".loopt/workflows/strings");
    let steps = parsed["jobs"]["build"]["steps"]
        .as_array()
        .expect("steps array");

    assert_eq!(steps[0]["run"].as_str().unwrap(), shell_script);
    assert_eq!(steps[1]["name"].as_str().unwrap(), backtick_value);
    assert_eq!(steps[2]["name"].as_str().unwrap(), unicode_value);
    assert_eq!(steps[3]["name"].as_str().unwrap(), control_value);
}

/// Integers must survive exactly, including values beyond `i64::MAX` that
/// only fit in `u64` (Go's default `int` is 64-bit, so such a value needs
/// an explicit `uint64(...)` conversion in the generated source or it
/// would either fail to compile or silently lose precision).
#[test]
fn integers_are_exact_across_the_u64_range() {
    let small = 42i64;
    let negative = -1_234_567_890i64;
    let max_i64 = i64::MAX;
    let max_u64 = u64::MAX; // 18446744073709551615, exceeds i64::MAX

    let workflow = object(vec![
        ("small", Value::Number(Number::from(small))),
        ("negative", Value::Number(Number::from(negative))),
        ("max_i64", Value::Number(Number::from(max_i64))),
        ("max_u64", Value::Number(Number::from(max_u64))),
    ]);

    let (stdout, parsed) = run_workflow(&workflow, ".loopt/workflows/integers");

    assert_eq!(parsed["small"].as_i64().unwrap(), small);
    assert_eq!(parsed["negative"].as_i64().unwrap(), negative);
    assert_eq!(parsed["max_i64"].as_i64().unwrap(), max_i64);
    assert_eq!(parsed["max_u64"].as_u64().unwrap(), max_u64);

    // Exact textual check too: a round-trip through f64 would corrupt
    // u64::MAX (it would print as 18446744073709551616).
    assert!(
        stdout.contains("18446744073709551615"),
        "u64::MAX must appear with exact digits, not a float-rounded value: {stdout}"
    );
}

#[test]
fn only_main_go_in_a_go_project_is_an_entrypoint() {
    let scratch = ScratchDir::new("detect");
    let project = scratch.path().join(".loopt/workflows/ci");
    fs::create_dir_all(&project).expect("create project dir");
    fs::write(project.join("go.mod"), "module ci\n\ngo 1.21\n").expect("write go.mod");
    fs::write(project.join("main.go"), "package main\n").expect("write main.go");
    fs::write(project.join("helper.go"), "package main\n").expect("write helper.go");

    assert_eq!(
        Language::detect_entrypoint(&project.join("main.go")),
        Some(Language::Go)
    );
    assert_eq!(
        Language::detect_entrypoint(&project.join("helper.go")),
        None,
        "helper files inside a Go project must not be separate workflows"
    );
}

#[test]
fn command_rejects_non_go_source() {
    let err = go::command(Path::new(".loopt/workflows/ci.py"))
        .expect_err("command must reject a non-.go path");
    assert!(err.to_string().contains(".go"));
}

#[test]
fn command_rejects_a_standalone_single_file_source() {
    let err = go::command(Path::new(".loopt/workflows/ci.go"))
        .expect_err("command must reject a legacy single-file source");
    assert!(
        err.to_string().contains("main.go"),
        "error should name the project entrypoint: {err}"
    );
}

#[test]
fn command_runs_the_project_directory() {
    let spec =
        go::command(Path::new(".loopt/workflows/ci/main.go")).expect("command should succeed");
    assert_eq!(spec.program, "go");
    assert_eq!(
        spec.args,
        vec!["-C", ".loopt/workflows/ci", "run", "."],
        "command must change into the project and run its package"
    );
}

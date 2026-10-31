//! Deterministic, fast CLI behavioral tests. Unlike `corpus.rs` (which
//! exercises every real Preloop fixture), these tests use small,
//! purpose-built inputs authored here -- never copied from the Preloop
//! fixture corpus -- to pin down specific `loopt import [PATHS...]
//! --language <id> [--force]` / `loopt gen [SOURCES...] [--force]`
//! behaviors: from-scratch generation, a source edit producing a changed
//! YAML, overwrite/conflict refusal and its explicit `--force` override,
//! path traversal and symlink-escape rejection, and rejection of a
//! generator process that doesn't emit exactly one JSON object.
//!
//! Every test runs the real `loopt` binary against a real language runtime
//! (`node`, `python3`, `go run`, or `cargo run`) and inspects real files it
//! wrote (or, for negative cases, real files it refused to touch). None of
//! this fakes a language process by echoing a hardcoded value: the
//! hand-authored sources below do real work (looping over a list to build
//! steps) and are actually executed.

mod support;

use support::{assert_order_preserving_eq, parse_document, TestRepo};

// ---------------------------------------------------------------------
// From-scratch generation: no `import` involved, a hand-authored source
// file per language, executed by `loopt gen` to produce YAML that didn't
// exist before.
// ---------------------------------------------------------------------

const TS_SCRATCH_SOURCE: &str = r#"
const messages: string[] = ["start", "end"];
const steps = messages.map((m, i) => ({ name: `step-${i}`, run: `echo ${m}` }));
const workflow = {
  name: "scratch",
  on: "push",
  jobs: { build: { "runs-on": "ubuntu-latest", steps } },
};
console.log(JSON.stringify(workflow));
"#;

const PY_SCRATCH_SOURCE: &str = r#"
import json

messages = ["start", "end"]
steps = [{"name": f"step-{i}", "run": f"echo {m}"} for i, m in enumerate(messages)]
workflow = {
    "name": "scratch",
    "on": "push",
    "jobs": {"build": {"runs-on": "ubuntu-latest", "steps": steps}},
}
print(json.dumps(workflow))
"#;

const GO_SCRATCH_SOURCE: &str = r#"package main

import (
	"encoding/json"
	"fmt"
	"os"
)

func main() {
	messages := []string{"start", "end"}
	steps := make([]map[string]interface{}, 0, len(messages))
	for i, m := range messages {
		steps = append(steps, map[string]interface{}{
			"name": fmt.Sprintf("step-%d", i),
			"run":  fmt.Sprintf("echo %s", m),
		})
	}
	workflow := map[string]interface{}{
		"name": "scratch",
		"on":   "push",
		"jobs": map[string]interface{}{
			"build": map[string]interface{}{
				"runs-on": "ubuntu-latest",
				"steps":   steps,
			},
		},
	}
	enc := json.NewEncoder(os.Stdout)
	if err := enc.Encode(workflow); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
"#;

const RUST_SCRATCH_MANIFEST: &str = r#"[package]
name = "scratch_rs"
version = "0.0.0"
edition = "2021"

[[bin]]
name = "scratch_rs"
path = "main.rs"

[workspace]
"#;

const RUST_SCRATCH_SOURCE: &str = r#"fn main() {
    let messages = ["start", "end"];
    let mut steps = String::new();
    for (i, m) in messages.iter().enumerate() {
        if i > 0 {
            steps.push(',');
        }
        steps.push_str(&format!(
            "{{\"name\":\"step-{i}\",\"run\":\"echo {m}\"}}"
        ));
    }
    println!(
        "{{\"name\":\"scratch\",\"on\":\"push\",\"jobs\":{{\"build\":{{\"runs-on\":\"ubuntu-latest\",\"steps\":[{steps}]}}}}}}"
    );
}
"#;

/// Writes `messages` into a from-scratch Python source (fast: no compile
/// step), driven by a loop so the step count genuinely depends on the
/// input rather than being a fixed literal.
fn write_py_scratch(repo: &TestRepo, rel: &str, messages: &[&str]) {
    let items: String = messages
        .iter()
        .map(|m| format!("{m:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!(
        "import json\n\nmessages = [{items}]\n\
         steps = [{{\"name\": f\"step-{{i}}\", \"run\": f\"echo {{m}}\"}} for i, m in enumerate(messages)]\n\
         workflow = {{\n    \"name\": \"scratch\",\n    \"on\": \"push\",\n    \
         \"jobs\": {{\"build\": {{\"runs-on\": \"ubuntu-latest\", \"steps\": steps}}}},\n}}\n\
         print(json.dumps(workflow))\n"
    );
    repo.write(rel, &source);
}

#[test]
fn from_scratch_generation_typescript() {
    let repo = TestRepo::new("scratch-ts");
    repo.write(".loopt/workflows/scratch-ts.ts", TS_SCRATCH_SOURCE);

    repo.run(&["gen", ".loopt/workflows/scratch-ts.ts"])
        .assert_success();

    assert!(
        repo.exists(".github/workflows/scratch-ts.yml"),
        "expected .github/workflows/scratch-ts.yml to be created from scratch"
    );
    let doc = parse_document(&repo.path(".github/workflows/scratch-ts.yml"))
        .expect("generated YAML parses");
    let steps = doc.value()["jobs"]["build"]["steps"]
        .as_array()
        .expect("steps is an array");
    assert_eq!(
        steps.len(),
        2,
        "scratch source builds steps from a 2-item loop"
    );

    let generated = repo.read(".github/workflows/scratch-ts.yml");
    assert!(
        generated.starts_with("# Generated by loopt."),
        "generated YAML must advertise that it is autogenerated: {generated}"
    );
    repo.run(&["gen", ".loopt/workflows/scratch-ts.ts"])
        .assert_success();
    assert_eq!(
        repo.read(".github/workflows/scratch-ts.yml"),
        generated,
        "a second generation must not duplicate or rewrite the header"
    );
}

#[test]
fn from_scratch_generation_python() {
    let repo = TestRepo::new("scratch-py");
    repo.write(".loopt/workflows/scratch-py.py", PY_SCRATCH_SOURCE);

    repo.run(&["gen", ".loopt/workflows/scratch-py.py"])
        .assert_success();

    assert!(repo.exists(".github/workflows/scratch-py.yml"));
    let doc = parse_document(&repo.path(".github/workflows/scratch-py.yml"))
        .expect("generated YAML parses");
    assert_eq!(doc.value()["name"], "scratch");
    assert_eq!(
        doc.value()["jobs"]["build"]["steps"]
            .as_array()
            .expect("steps array")
            .len(),
        2
    );
}

#[test]
fn from_scratch_generation_go() {
    let repo = TestRepo::new("scratch-go");
    repo.write(".loopt/workflows/scratch-go.go", GO_SCRATCH_SOURCE);

    repo.run(&["gen", ".loopt/workflows/scratch-go.go"])
        .assert_success();

    assert!(repo.exists(".github/workflows/scratch-go.yml"));
    let doc = parse_document(&repo.path(".github/workflows/scratch-go.yml"))
        .expect("generated YAML parses");
    assert_eq!(
        doc.value()["jobs"]["build"]["steps"]
            .as_array()
            .expect("steps array")
            .len(),
        2
    );
}

#[test]
fn from_scratch_generation_rust() {
    let repo = TestRepo::new("scratch-rust");
    repo.write(
        ".loopt/workflows/scratch-rs/Cargo.toml",
        RUST_SCRATCH_MANIFEST,
    );
    repo.write(".loopt/workflows/scratch-rs/main.rs", RUST_SCRATCH_SOURCE);

    repo.run(&["gen", ".loopt/workflows/scratch-rs/main.rs"])
        .assert_success();

    assert!(repo.exists(".github/workflows/scratch-rs.yml"));
    let doc = parse_document(&repo.path(".github/workflows/scratch-rs.yml"))
        .expect("generated YAML parses");
    assert_eq!(
        doc.value()["jobs"]["build"]["steps"]
            .as_array()
            .expect("steps array")
            .len(),
        2
    );
}

// ---------------------------------------------------------------------
// Source mutation yields changed YAML.
// ---------------------------------------------------------------------

#[test]
fn source_mutation_yields_changed_yaml() {
    let repo = TestRepo::new("mutate");
    let source_rel = ".loopt/workflows/mutate.py";
    let output_rel = ".github/workflows/mutate.yml";

    write_py_scratch(&repo, source_rel, &["start"]);
    repo.run(&["gen", source_rel]).assert_success();
    let before = parse_document(&repo.path(output_rel)).expect("first generation parses");
    let before_steps = before.value()["jobs"]["build"]["steps"]
        .as_array()
        .expect("steps array")
        .len();
    assert_eq!(before_steps, 1);

    // Mutate the source -- a real edit a user would make, not a
    // hand-crafted expected-output stub.
    write_py_scratch(&repo, source_rel, &["start", "middle", "end"]);
    repo.run(&["gen", source_rel]).assert_success();
    let after = parse_document(&repo.path(output_rel)).expect("second generation parses");
    let after_steps = after.value()["jobs"]["build"]["steps"]
        .as_array()
        .expect("steps array")
        .len();
    assert_eq!(
        after_steps, 3,
        "regenerated YAML must reflect the mutated source's 3-item loop"
    );
    assert_ne!(
        before_steps, after_steps,
        "source mutation must yield a genuinely different generated YAML"
    );
}

// ---------------------------------------------------------------------
// Explicit overwrite / conflict behavior: `gen` refuses to clobber YAML
// that changed on disk since the last accepted generation, and `--force`
// (or re-`import`ing the edited YAML) is required to move past that.
// ---------------------------------------------------------------------

#[test]
fn gen_refuses_externally_edited_yaml_without_force() {
    let repo = TestRepo::new("conflict");
    let source_rel = ".loopt/workflows/conflict.py";
    let output_rel = ".github/workflows/conflict.yml";

    write_py_scratch(&repo, source_rel, &["start"]);
    repo.run(&["gen", source_rel]).assert_success();
    let generated_once = repo.read(output_rel);

    // Simulate a user hand-editing the generated YAML directly.
    let externally_edited = format!("{generated_once}\n# hand-edited by a human\n");
    repo.write(output_rel, &externally_edited);

    let result = repo.run(&["gen", source_rel]);
    result.assert_failure();
    assert_eq!(
        repo.read(output_rel),
        externally_edited,
        "a refused `gen` must leave the externally-edited YAML byte-for-byte untouched"
    );
}

#[test]
fn gen_force_overwrites_externally_edited_yaml() {
    let repo = TestRepo::new("conflict-force");
    let source_rel = ".loopt/workflows/conflict-force.py";
    let output_rel = ".github/workflows/conflict-force.yml";

    write_py_scratch(&repo, source_rel, &["start"]);
    repo.run(&["gen", source_rel]).assert_success();
    let generated_once = repo.read(output_rel);
    let pristine = preloop_gha_parser::WorkflowDocument::parse(&generated_once)
        .expect("first generation parses");
    assert!(
        generated_once.contains("echo start"),
        "scratch YAML should contain the generated source's run command"
    );
    let externally_edited = generated_once.replace("echo start", "echo human-edit");
    repo.write(output_rel, &externally_edited);

    // Default `gen` must have refused this edit (covered by the sibling
    // test); `--force` is the explicit override.
    repo.run(&["gen", "--force", source_rel]).assert_success();

    assert_ne!(
        repo.read(output_rel),
        externally_edited,
        "`gen --force` must overwrite the externally-edited YAML"
    );
    // The source never changed, so a forced regeneration must reproduce
    // exactly the same document as the original (pre-edit) generation.
    let regenerated = parse_document(&repo.path(output_rel)).expect("forced output parses");
    assert_order_preserving_eq(pristine.value(), regenerated.value());
}

#[test]
fn import_without_force_refuses_to_clobber_existing_source() {
    let repo = TestRepo::new("import-collision");
    let workflow_rel = ".github/workflows/import-collision.yml";
    repo.write(
        workflow_rel,
        "name: original\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo original\n",
    );

    repo.run(&["import", workflow_rel, "--language", "python"])
        .assert_success();
    let source_rel = ".loopt/workflows/import-collision.py";
    assert!(
        repo.exists(source_rel),
        "first import creates a managed source"
    );
    let first_source = repo.read(source_rel);

    // A user hand-edits the authored program, then re-imports the same YAML
    // without acknowledging that the existing source is no longer the last
    // source Loopt generated.
    let edited_source = format!("{first_source}\n# hand-edited by a human\n");
    repo.write(source_rel, &edited_source);
    let result = repo.run(&["import", workflow_rel, "--language", "python"]);
    result.assert_failure();
    assert_eq!(
        repo.read(source_rel),
        edited_source,
        "a refused re-import must not touch the hand-edited authored source"
    );

    repo.run(&["import", workflow_rel, "--language", "python", "--force"])
        .assert_success();
    assert_eq!(
        repo.read(source_rel),
        first_source,
        "`import --force` must overwrite the hand-edited source from YAML"
    );
}

// ---------------------------------------------------------------------
// Path traversal / symlink escapes.
// ---------------------------------------------------------------------

#[test]
fn import_rejects_path_outside_repo_root() {
    let repo = TestRepo::new("escape-import");
    let outside = TestRepo::new("escape-import-outside");
    outside.write(
        "evil.yml",
        "name: evil\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo evil\n",
    );
    let outside_before = outside.list_files(".");

    let outside_dir_name = outside
        .root()
        .file_name()
        .expect("temp dir has a name")
        .to_string_lossy()
        .into_owned();
    let traversal_arg = format!("../{outside_dir_name}/evil.yml");

    let result = repo.run(&["import", &traversal_arg, "--language", "python"]);
    result.assert_failure();
    assert!(
        repo.list_files(".loopt/workflows").is_empty(),
        "a rejected traversal import must not write anything inside the repo"
    );
    assert_eq!(
        outside.list_files("."),
        outside_before,
        "a rejected traversal import must not write generated files into a \
         directory outside the repo root either"
    );
}

#[test]
fn gen_rejects_source_path_outside_repo_root() {
    let repo = TestRepo::new("escape-gen");
    let outside = TestRepo::new("escape-gen-outside");
    write_py_scratch(&outside, "evil.py", &["x"]);
    let outside_before = outside.list_files(".");

    let outside_dir_name = outside
        .root()
        .file_name()
        .expect("temp dir has a name")
        .to_string_lossy()
        .into_owned();
    let traversal_arg = format!("../{outside_dir_name}/evil.py");

    let result = repo.run(&["gen", &traversal_arg]);
    result.assert_failure();
    assert!(
        repo.list_files(".github/workflows").is_empty(),
        "a rejected traversal gen must not write anything inside the repo"
    );
    assert_eq!(
        outside.list_files("."),
        outside_before,
        "a rejected traversal gen must not write YAML into a directory outside \
         the repo root either"
    );
}

#[test]
fn gen_refuses_to_write_through_symlinked_output() {
    let repo = TestRepo::new("escape-symlink");
    let outside = TestRepo::new("escape-symlink-outside");
    let outside_target = outside.write("captured.yml", "sentinel\n");
    let outside_before = outside.read("captured.yml");

    let source_rel = ".loopt/workflows/escape-symlink.py";
    write_py_scratch(&repo, source_rel, &["start"]);

    // The output destination is a symlink pointing outside the repo
    // *before* `gen` ever runs.
    repo.symlink(".github/workflows/escape-symlink.yml", &outside_target);

    let result = repo.run(&["gen", source_rel]);
    result.assert_failure();
    assert_eq!(
        outside.read("captured.yml"),
        outside_before,
        "gen must never follow a symlinked output destination outside the repo root"
    );
}

// ---------------------------------------------------------------------
// Duplicate outputs: two distinct sources with the same derived stem must
// not silently race to the same destination.
// ---------------------------------------------------------------------

#[test]
fn gen_rejects_duplicate_destination_from_two_sources() {
    let repo = TestRepo::new("dup-dest");
    write_py_scratch(&repo, ".loopt/workflows/dup.py", &["a"]);
    repo.write(".loopt/workflows/dup.ts", TS_SCRATCH_SOURCE);

    let result = repo.run(&["gen"]);
    result.assert_failure();
    assert!(
        !repo.exists(".github/workflows/dup.yml"),
        "a duplicate-destination collision must be caught before any write"
    );
}

// ---------------------------------------------------------------------
// Invalid JSON from the language process.
// ---------------------------------------------------------------------

#[test]
fn gen_rejects_non_json_stdout() {
    let repo = TestRepo::new("bad-stdout");
    repo.write(
        ".loopt/workflows/bad-stdout.py",
        "print(\"this is not JSON at all\")\n",
    );

    let result = repo.run(&["gen", ".loopt/workflows/bad-stdout.py"]);
    result.assert_failure();
    assert!(
        !result.stderr.is_empty(),
        "a generator-output failure must surface a diagnostic on stderr"
    );
    assert!(!repo.exists(".github/workflows/bad-stdout.yml"));
}

#[test]
fn gen_rejects_non_object_json_stdout() {
    let repo = TestRepo::new("bad-shape");
    repo.write(
        ".loopt/workflows/bad-shape.py",
        "import json\nprint(json.dumps([1, 2, 3]))\n",
    );

    let result = repo.run(&["gen", ".loopt/workflows/bad-shape.py"]);
    result.assert_failure();
    assert!(!repo.exists(".github/workflows/bad-shape.yml"));
}

#[test]
fn gen_rejects_nonzero_exit_process() {
    let repo = TestRepo::new("bad-exit");
    repo.write(
        ".loopt/workflows/bad-exit.py",
        "import sys\nprint(\"about to fail\", file=sys.stderr)\nsys.exit(1)\n",
    );

    let result = repo.run(&["gen", ".loopt/workflows/bad-exit.py"]);
    result.assert_failure();
    assert!(!repo.exists(".github/workflows/bad-exit.yml"));
}

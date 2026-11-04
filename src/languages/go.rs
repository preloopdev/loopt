//! Go language integration: emits an editable Go project from a workflow
//! document, and describes how to run it to recover that document.
//!
//! `emit` turns a parsed GitHub Actions workflow (`serde_json::Value`) into
//! a standalone Go project (`<stem>/go.mod` + `<stem>/main.go`) whose
//! `main.go` *constructs* the same value as native Go data — strings,
//! bools, numbers, `[]any` slices, and ordered objects from the vendored
//! SDK's `om` package — and prints it through that SDK's `emit.Print`.
//! Nothing is embedded as an opaque JSON/YAML string: every scalar, key and
//! nesting level is a real Go expression a user can edit, loop over, or
//! wrap in helper functions.
//!
//! Go's built-in `encoding/json` package marshals `map[string]any` keys in
//! sorted order, which would silently reorder a workflow's top-level keys,
//! a job's fields, or a matrix's axes. The SDK's `om.Object` preserves
//! insertion order and is used for every JSON object instead of a plain Go
//! map.
//!
//! `emit` also (re)writes a vendored copy of the `loopt-sdk` Go module under
//! the fixed, project-independent `.loopt/sdk/go`, embedded in this binary
//! at compile time (`SDK_FILES`, via `include_str!` from the repository's
//! own `sdk/go`), so every generated project's `go.mod` can require it with
//! a plain relative `replace` path instead of duplicating the SDK per
//! project or requiring it to be published. Every go-language document's
//! `GeneratedSource` repeats the same `.loopt/sdk/go/*` files verbatim;
//! `import`/`init` tolerate that specific kind of repeat (see
//! `paths::SDK_DIR`) since the vendored copy is identical byte-for-byte no
//! matter which document triggered writing it.

use std::path::{Component, Path};

use anyhow::{bail, Context, Result};
use serde_json::{Map, Number, Value};

use crate::languages::{CommandSpec, GeneratedFile, GeneratedSource};

const INDENT: &str = "\t";

/// Project-relative directory the vendored `loopt-sdk` Go module is written
/// to, shared by every generated Go workflow project in the repository.
const SDK_VENDOR_DIR: &str = ".loopt/sdk/go";

/// The vendored `loopt-sdk` Go module's import path.
const SDK_MODULE: &str = "github.com/preloopdev/loopt/sdk/go";

/// The vendored `loopt-sdk` Go module's sources, embedded at compile time
/// from `sdk/go` (built and tested as a normal Go module -- see
/// `sdk/go/go.mod`). Paths are relative to `SDK_VENDOR_DIR`.
const SDK_FILES: &[(&str, &str)] = &[
    ("go.mod", include_str!("../../sdk/go/go.mod")),
    ("om/om.go", include_str!("../../sdk/go/om/om.go")),
    ("om/om_test.go", include_str!("../../sdk/go/om/om_test.go")),
    ("expr/expr.go", include_str!("../../sdk/go/expr/expr.go")),
    (
        "expr/contexts.go",
        include_str!("../../sdk/go/expr/contexts.go"),
    ),
    (
        "expr/literal.go",
        include_str!("../../sdk/go/expr/literal.go"),
    ),
    (
        "expr/operators.go",
        include_str!("../../sdk/go/expr/operators.go"),
    ),
    (
        "expr/functions.go",
        include_str!("../../sdk/go/expr/functions.go"),
    ),
    (
        "expr/expr_test.go",
        include_str!("../../sdk/go/expr/expr_test.go"),
    ),
    ("core/core.go", include_str!("../../sdk/go/core/core.go")),
    ("core/step.go", include_str!("../../sdk/go/core/step.go")),
    ("core/job.go", include_str!("../../sdk/go/core/job.go")),
    (
        "core/workflow.go",
        include_str!("../../sdk/go/core/workflow.go"),
    ),
    (
        "core/core_test.go",
        include_str!("../../sdk/go/core/core_test.go"),
    ),
    ("emit/emit.go", include_str!("../../sdk/go/emit/emit.go")),
    (
        "emit/emit_test.go",
        include_str!("../../sdk/go/emit/emit_test.go"),
    ),
];

/// The vendored `loopt-sdk` sources as `GeneratedFile`s rooted at
/// `SDK_VENDOR_DIR`, appended to every emitted Go `GeneratedSource`.
fn sdk_files() -> Vec<GeneratedFile> {
    SDK_FILES
        .iter()
        .map(|(relative, content)| GeneratedFile {
            path: Path::new(SDK_VENDOR_DIR).join(relative),
            content: (*content).to_string(),
        })
        .collect()
}

/// Build the standalone Go project for `workflow`, returning a
/// `GeneratedSource` rooted at `<stem>/main.go` with a sibling
/// `<stem>/go.mod`, plus the vendored `loopt-sdk` module under
/// `.loopt/sdk/go` that it requires.
pub fn emit(workflow: &Value, stem: &Path) -> Result<GeneratedSource> {
    let main_path = stem.join("main.go");

    let mut source = String::with_capacity(1024);
    source.push_str(&render_header(contains_float(workflow)));
    source.push_str("func workflow() any {\n");
    source.push_str(INDENT);
    source.push_str("return ");
    render_value(workflow, 1, &mut source);
    source.push('\n');
    source.push_str("}\n\n");
    source.push_str(MAIN_FN);

    let mut files = vec![
        GeneratedFile {
            path: stem.join("go.mod"),
            content: render_manifest(stem)?,
        },
        GeneratedFile {
            path: main_path.clone(),
            content: source,
        },
    ];
    files.extend(sdk_files());

    Ok(GeneratedSource {
        entrypoint: main_path,
        files,
    })
}

/// Describe how to run a previously generated Go project: `go -C <project>
/// run .`, executed with no shell and no external modules, from the
/// repository root. The project directory is the entrypoint's parent.
pub fn command(source: &Path) -> Result<CommandSpec> {
    match source.extension().and_then(|ext| ext.to_str()) {
        Some("go") => {}
        _ => bail!("go: expected a .go source file, got {}", source.display()),
    }
    if source.file_name().and_then(|name| name.to_str()) != Some("main.go") {
        bail!(
            "go: expected a generated project entrypoint named main.go, got {}",
            source.display()
        );
    }

    let project_dir = source
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let directory = project_dir
        .to_str()
        .with_context(|| {
            format!(
                "go: project directory {} is not valid UTF-8",
                project_dir.display()
            )
        })?
        .to_string();

    Ok(CommandSpec {
        program: "go".to_string(),
        args: vec![
            "-C".to_string(),
            directory,
            "run".to_string(),
            ".".to_string(),
        ],
        // A repository-level `go.work` lists member modules that a partial
        // checkout (or a vendored SDK inside `.loopt`) does not contain, and
        // `go run` would fail before our program starts. The generated
        // project is self-contained, so opt out of the ambient workspace.
        env: vec![("GOWORK".to_string(), "off".to_string())],
    })
}

/// Whether `value` contains a JSON number that must survive as a float
/// literal (`json.Number`), which decides if the generated file needs to
/// import `encoding/json`.
fn contains_float(value: &Value) -> bool {
    match value {
        Value::Number(n) => n.is_f64(),
        Value::Array(items) => items.iter().any(contains_float),
        Value::Object(map) => map.values().any(contains_float),
        _ => false,
    }
}

/// Render the generated project's header and imports.
fn render_header(needs_json: bool) -> String {
    format!(
        "// Code generated by `loopt import`. This file is yours to edit: fields\n\
         // below are ordinary Go values (strings, bools, numbers, slices, and\n\
         // ordered objects built with the SDK's om.New/om.F), so add a step with\n\
         // a loop, read an environment variable to pick a runner, or pull shared\n\
         // values into a function -- anything a normal Go program can do. Run\n\
         // `loopt gen` to turn the result back into a workflow YAML file.\n\
         package main\n\
         \n\
         import (\n\
         {json_import}\t\"fmt\"\n\
         \t\"os\"\n\
         \n\
         \t\"{}/emit\"\n\
         \t\"{}/om\"\n\
         )\n\
         \n",
        SDK_MODULE,
        SDK_MODULE,
        json_import = if needs_json {
            "\t\"encoding/json\"\n"
        } else {
            ""
        },
    )
}

const MAIN_FN: &str = "\
func main() {
\tif err := emit.Print(workflow()); err != nil {
\t\tfmt.Fprintln(os.Stderr, \"loopt: failed to encode workflow:\", err)
\t\tos.Exit(1)
\t}
}
";

/// Render the generated `go.mod`. The module path is derived from the
/// project's stem, and the SDK requirement carries a relative `replace` so
/// the project builds entirely offline against the vendored copy.
fn render_manifest(stem: &Path) -> Result<String> {
    let module_name = safe_module_name(stem);
    let replace_path = sdk_replace_path(stem)?;
    Ok(format!(
        "// Generated by `loopt import`. Edit freely; regenerate the workflow YAML with `loopt gen`.\n\
         module {}\n\
         \n\
         go 1.21\n\
         \n\
         require {} v0.0.0\n\
         \n\
         replace {} => {}\n",
        module_name, SDK_MODULE, SDK_MODULE, replace_path,
    ))
}

/// Derive a valid, lowercase module path from the final component of
/// `stem`: ASCII alphanumerics are lowercased and kept, everything else
/// becomes `_`, and the result is prefixed with `wf_` if it would not
/// otherwise start with a letter.
fn safe_module_name(stem: &Path) -> String {
    let raw = stem
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workflow");

    let mut name: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();

    if name.is_empty() {
        name = "workflow".to_string();
    }
    if !name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        name = format!("wf_{name}");
    }
    format!("loopt.local/workflows/{name}")
}

/// Render the relative path from `stem` (the generated project directory)
/// to the vendored SDK module, e.g. `../../sdk/go` for
/// `.loopt/workflows/ci` and `../../../../sdk/go` for a nested action
/// project.
fn sdk_replace_path(stem: &Path) -> Result<String> {
    let stem_components = normal_components(stem)?;
    let target_components = normal_components(Path::new(SDK_VENDOR_DIR))?;

    let common = stem_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(stem_component, target_component)| stem_component == target_component)
        .count();

    let mut parts: Vec<&str> = Vec::new();
    parts.extend(std::iter::repeat_n("..", stem_components.len() - common));
    parts.extend(target_components[common..].iter().copied());

    if parts.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(parts.join("/"))
    }
}

/// The plain path components of a project-relative directory, rejecting
/// anything that could escape the project.
fn normal_components(path: &Path) -> Result<Vec<&str>> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => components.push(name.to_str().with_context(|| {
                format!(
                    "go: path component of {} is not valid UTF-8",
                    path.display()
                )
            })?),
            Component::CurDir => {}
            other => bail!(
                "go: generated project path {} contains an unsupported component {:?}",
                path.display(),
                other
            ),
        }
    }
    Ok(components)
}

fn render_value(value: &Value, depth: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("nil"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => render_number(n, out),
        Value::String(s) => render_string(s, out),
        Value::Array(items) => render_array(items, depth, out),
        Value::Object(map) => render_object(map, depth, out),
    }
}

/// Renders a JSON number as a Go numeric literal. Values that fit `i64`
/// (Go's default `int` is 64-bit on every platform Loopt targets) are
/// emitted as plain literals; values that only fit `u64` are given an
/// explicit `uint64(...)` conversion so the full unsigned range survives
/// without overflowing Go's untyped-constant default type. `serde_json`'s
/// number text is already valid Go numeral syntax (JSON numeral grammar is
/// a subset of Go's), so floats are emitted verbatim.
fn render_number(n: &Number, out: &mut String) {
    if n.as_u64().is_some() && n.as_i64().is_none() {
        out.push_str("uint64(");
        out.push_str(&n.to_string());
        out.push(')');
        return;
    }
    // Floats go through `json.Number` so Go's encoder emits the literal
    // text verbatim. A bare Go float literal would be renormalized on
    // marshal (`2.0` -> `2`), silently changing an env value that scripts
    // read as a string.
    if n.is_f64() {
        out.push_str("json.Number(\"");
        out.push_str(&n.to_string());
        out.push_str("\")");
        return;
    }
    out.push_str(&n.to_string());
}

/// Renders a JSON string as a Go string literal. Prefers a raw
/// (backtick-delimited) literal when safe -- the common case for
/// multi-line shell scripts, which then read as an actual multi-line
/// block instead of a wall of `\n` escapes. Falls back to an interpreted
/// literal (escaping control characters) whenever the content contains a
/// backtick or carriage return, both of which raw literals cannot
/// represent exactly (Go strips `\r` from raw literals, and a literal
/// backtick would terminate them early). Ordinary Unicode text is kept as
/// literal UTF-8 either way.
fn render_string(s: &str, out: &mut String) {
    if can_use_raw_literal(s) {
        out.push('`');
        out.push_str(s);
        out.push('`');
    } else {
        render_interpreted_string(s, out);
    }
}

fn can_use_raw_literal(s: &str) -> bool {
    !s.contains('`') && !s.contains('\r')
}

fn render_interpreted_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn render_array(items: &[Value], depth: usize, out: &mut String) {
    if items.is_empty() {
        out.push_str("[]any{}");
        return;
    }
    out.push_str("[]any{\n");
    for item in items {
        push_indent(out, depth + 1);
        render_value(item, depth + 1, out);
        out.push_str(",\n");
    }
    push_indent(out, depth);
    out.push('}');
}

fn render_object(map: &Map<String, Value>, depth: usize, out: &mut String) {
    if map.is_empty() {
        out.push_str("om.New()");
        return;
    }
    out.push_str("om.New(\n");
    for (key, value) in map {
        push_indent(out, depth + 1);
        out.push_str("om.F(");
        render_string(key, out);
        out.push_str(", ");
        render_value(value, depth + 1, out);
        out.push_str("),\n");
    }
    push_indent(out, depth);
    out.push(')');
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str(INDENT);
    }
}

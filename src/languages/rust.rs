//! Rust language integration.
//!
//! `emit` turns a parsed GitHub Actions workflow (`serde_json::Value`) into a
//! standalone Cargo project (`<stem>/Cargo.toml` + `<stem>/main.rs`) whose
//! `main.rs` *constructs* the same value as native `serde_json::Value` data
//! (`obj`/`arr`/`num` calls, `Value::String`, `Value::Bool`, `Value::Null`)
//! and prints it back out as one JSON object on stdout. Nothing is embedded
//! as an opaque JSON/YAML string: every scalar, key and nesting level is a
//! real Rust expression that a user can edit, loop over, or wrap in helper
//! functions. The generated project is a self-contained Cargo workspace
//! (`[workspace]` with no members but itself) so it never gets swallowed by
//! an ancestor workspace, and it is built/run with an explicit
//! `--target-dir` under `.loopt/.cache` so build artifacts never land inside
//! `.loopt/workflows`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Number, Value};

use crate::languages::{CommandSpec, GeneratedFile, GeneratedSource};

const INDENT: &str = "    ";

/// Build the standalone Rust/Cargo source for `workflow`, returning a
/// `GeneratedSource` rooted at `<stem>/main.rs` with a sibling
/// `<stem>/Cargo.toml`.
pub fn emit(workflow: &Value, stem: &Path) -> Result<GeneratedSource> {
    let crate_name = safe_crate_name(stem);

    let mut literal = String::new();
    write_value(&mut literal, workflow, 1);

    let main_path = stem.join("main.rs");
    let manifest_path = stem.join("Cargo.toml");

    Ok(GeneratedSource {
        entrypoint: main_path.clone(),
        files: vec![
            GeneratedFile {
                path: manifest_path,
                content: render_manifest(&crate_name),
            },
            GeneratedFile {
                path: main_path,
                content: render_main(&literal),
            },
        ],
    })
}

/// Describe how to build and run a previously generated Rust source file
/// via `cargo run`, using an explicit `--manifest-path` and a project-scoped
/// `--target-dir` under `.loopt/.cache` so cargo never writes build
/// artifacts into `.loopt/workflows` and never needs a globally-installed
/// binary beyond whatever `cargo` the caller's `PATH` resolves to.
pub fn command(source: &Path) -> Result<CommandSpec> {
    let project_dir = source.parent().with_context(|| {
        format!(
            "rust source path {} has no parent project directory",
            source.display()
        )
    })?;

    let manifest_path = project_dir.join("Cargo.toml");
    let target_dir = cache_target_dir(project_dir);
    let crate_name = safe_crate_name(project_dir);

    Ok(CommandSpec {
        program: "cargo".to_string(),
        args: vec![
            "run".to_string(),
            "--quiet".to_string(),
            "--manifest-path".to_string(),
            path_to_arg(&manifest_path)?,
            "--target-dir".to_string(),
            path_to_arg(&target_dir)?,
            "--bin".to_string(),
            crate_name,
        ],
    })
}

/// Repository-scoped build cache rooted at `.loopt/.cache/rust-target`.
///
/// Every generated Rust workflow in the repository shares the compiled
/// dependency artifacts (serde_json is the only dependency), so importing a
/// hundred workflows does not rebuild serde_json a hundred times. Cargo
/// serializes concurrent builds on this directory itself.
fn cache_target_dir(_project_dir: &Path) -> PathBuf {
    Path::new(".loopt").join(".cache").join("rust-target")
}

fn path_to_arg(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .with_context(|| format!("path {} is not valid UTF-8", path.display()))
}

/// Derive a valid, lowercase Cargo package/binary name from the final
/// component of `stem` (or `project_dir`, which is the same path): ASCII
/// alphanumerics are lowercased and kept, everything else becomes `_`, and
/// the result is prefixed with `wf_` if it would not otherwise start with a
/// letter.
fn safe_crate_name(stem: &Path) -> String {
    let raw = stem
        .file_name()
        .and_then(|s| s.to_str())
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
    name
}

/// Render the generated `Cargo.toml`. `[workspace]` with no `members` key
/// makes this crate the root of its own single-package workspace, so cargo
/// never tries to fold it into an ancestor workspace it happens to be
/// nested under. `serde_json` is pinned with `preserve_order` (so
/// `serde_json::Map` iterates in insertion order, matching the generated
/// `obj(...)` call order) and `arbitrary_precision` (so `Number::from_str`
/// round-trips integers and floats of any magnitude/precision exactly,
/// rather than coercing them through `f64`/`i64`/`u64`).
fn render_manifest(crate_name: &str) -> String {
    format!(
        "# Generated by `loopt import`. Edit freely; regenerate the workflow YAML with `loopt gen`.\n\
         [workspace]\n\
         \n\
         [package]\n\
         name = \"{crate_name}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         publish = false\n\
         \n\
         [[bin]]\n\
         name = \"{crate_name}\"\n\
         path = \"main.rs\"\n\
         \n\
         [dependencies]\n\
         serde_json = {{ version = \"1\", features = [\"preserve_order\", \"arbitrary_precision\"] }}\n"
    )
}

/// Wrap the top-level `Value` expression in an editable module: `obj`/
/// `arr`/`num` helper constructors, a `build_workflow()` function returning
/// the literal, and a `main()` that serializes it to stdout as one JSON
/// object.
fn render_main(literal: &str) -> String {
    format!(
        "{header}\n\nfn build_workflow() -> Value {{\n{indent}{literal}\n}}\n\n{main_fn}",
        header = MODULE_HEADER,
        indent = INDENT,
        literal = literal,
        main_fn = MAIN_FN,
    )
}

const MODULE_HEADER: &str = "// Generated by `loopt import`.\n\
//\n\
// Edit the serde_json::Value returned by build_workflow() below -- add\n\
// imports, helper functions, loops or conditionals as needed -- then run\n\
// `loopt gen` to regenerate the workflow YAML from this source.\n\
use serde_json::{Map, Number, Value};\n\
use std::str::FromStr;\n\
\n\
/// Build a JSON object from an ordered list of key/value pairs, preserving\n\
/// the order the pairs are given in (requires serde_json's \"preserve_order\"\n\
/// feature, enabled in this project's Cargo.toml).\n\
fn obj(entries: Vec<(&str, Value)>) -> Value {\n\
    let mut map = Map::new();\n\
    for (key, value) in entries {\n\
        map.insert(key.to_string(), value);\n\
    }\n\
    Value::Object(map)\n\
}\n\
\n\
/// Build a JSON array from a list of values, in order.\n\
fn arr(items: Vec<Value>) -> Value {\n\
    Value::Array(items)\n\
}\n\
\n\
/// Build a JSON number from its exact textual representation (requires\n\
/// serde_json's \"arbitrary_precision\" feature, enabled in this project's\n\
/// Cargo.toml, so integers and floats of any magnitude/precision round-trip\n\
/// exactly instead of being coerced through f64/i64/u64).\n\
fn num(repr: &str) -> Value {\n\
    Value::Number(Number::from_str(repr).expect(\"valid JSON number literal\"))\n\
}";

const MAIN_FN: &str = "fn main() {\n\
    let workflow = build_workflow();\n\
    let rendered = serde_json::to_string(&workflow).expect(\"serialize workflow to JSON\");\n\
    println!(\"{rendered}\");\n\
}\n";

/// Recursively render `value` as a Rust expression built from the `obj`/
/// `arr`/`num` helpers (plus `Value::String`/`Value::Bool`/`Value::Null`
/// directly). `indent` is the nesting depth (in `INDENT` units) of the
/// contents of the expression being written; the returned text has no
/// leading indentation of its own, so the caller can splice it directly
/// after `return`, a key in a tuple, or inside a `vec![...]`.
fn write_value(out: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Null => out.push_str("Value::Null"),
        Value::Bool(true) => out.push_str("Value::Bool(true)"),
        Value::Bool(false) => out.push_str("Value::Bool(false)"),
        Value::Number(n) => write_number(out, n),
        Value::String(s) => {
            out.push_str("Value::String(");
            out.push_str(&rust_str_literal(s));
            out.push_str(".to_string())");
        }
        Value::Array(items) => write_array(out, items, indent),
        Value::Object(map) => write_object(out, map, indent),
    }
}

fn write_number(out: &mut String, n: &Number) {
    out.push_str("num(");
    out.push_str(&rust_str_literal(&n.to_string()));
    out.push(')');
}

fn write_array(out: &mut String, items: &[Value], indent: usize) {
    if items.is_empty() {
        out.push_str("arr(vec![])");
        return;
    }
    out.push_str("arr(vec![\n");
    for item in items {
        out.push_str(&INDENT.repeat(indent + 1));
        write_value(out, item, indent + 1);
        out.push_str(",\n");
    }
    out.push_str(&INDENT.repeat(indent));
    out.push_str("])");
}

fn write_object(out: &mut String, map: &Map<String, Value>, indent: usize) {
    if map.is_empty() {
        out.push_str("obj(vec![])");
        return;
    }
    out.push_str("obj(vec![\n");
    for (key, val) in map {
        out.push_str(&INDENT.repeat(indent + 1));
        out.push('(');
        out.push_str(&rust_str_literal(key));
        out.push_str(", ");
        write_value(out, val, indent + 1);
        out.push_str("),\n");
    }
    out.push_str(&INDENT.repeat(indent));
    out.push_str("])");
}

/// Render `s` as a double-quoted Rust string literal. Printable characters
/// (including non-ASCII Unicode) are emitted verbatim since generated
/// sources are UTF-8 `.rs` files; `\`, `"` and control characters are
/// escaped so the result is always valid, unambiguous Rust source.
fn rust_str_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

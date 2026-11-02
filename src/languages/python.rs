//! Python language integration.
//!
//! `emit` turns a parsed GitHub Actions workflow (`serde_json::Value`) into a
//! standalone, human-editable Python source file that *constructs* the same
//! value as native Python data (dicts, lists, strings, bools, `None`,
//! `int`/`float`) and prints it back out as one JSON object on stdout
//! through the vendored `loopt_sdk` package. Nothing is embedded as an
//! opaque JSON/YAML string: every scalar, key and nesting level is a real
//! Python literal that a user can edit, loop over, or wrap in helper
//! functions.
//!
//! `emit` also (re)writes a vendored copy of the Python SDK under the fixed,
//! project-independent `.loopt/sdk/python`, embedded in this binary at
//! compile time (`SDK_FILES`, via `include_str!` from the repository's own
//! `sdk/python`) so every generated source can import it with a plain
//! relative `sys.path` entry instead of requiring a `pip install`. Every
//! python-language document's `GeneratedSource` repeats the same
//! `.loopt/sdk/python/*` files verbatim; `import`/`init` tolerate that
//! specific kind of repeat (see `paths::SDK_DIR`) since the vendored copy is
//! identical byte-for-byte no matter which document triggered writing it.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Number, Value};

use crate::languages::{CommandSpec, GeneratedFile, GeneratedSource};

const INDENT: &str = "    ";

/// Project-relative directory the vendored `loopt_sdk` package is written
/// to, shared by every generated Python workflow source in the repository.
const SDK_VENDOR_DIR: &str = ".loopt/sdk/python";

/// The vendored Python SDK's package files, embedded at compile time from
/// `sdk/python` (tested with `python3 -m unittest discover`). Paths are
/// relative to `SDK_VENDOR_DIR`; `pyproject.toml`, `README.md` and tests are
/// intentionally not vendored because generated sources never need them.
const SDK_FILES: &[(&str, &str)] = &[
    (
        "loopt_sdk/__init__.py",
        include_str!("../../sdk/python/loopt_sdk/__init__.py"),
    ),
    (
        "loopt_sdk/core.py",
        include_str!("../../sdk/python/loopt_sdk/core.py"),
    ),
    (
        "loopt_sdk/emit.py",
        include_str!("../../sdk/python/loopt_sdk/emit.py"),
    ),
    (
        "loopt_sdk/errors.py",
        include_str!("../../sdk/python/loopt_sdk/errors.py"),
    ),
    (
        "loopt_sdk/expr.py",
        include_str!("../../sdk/python/loopt_sdk/expr.py"),
    ),
];

/// The vendored SDK sources as `GeneratedFile`s rooted at
/// `SDK_VENDOR_DIR`, appended to every emitted Python `GeneratedSource`.
fn sdk_files() -> Vec<GeneratedFile> {
    SDK_FILES
        .iter()
        .map(|(relative, content)| GeneratedFile {
            path: Path::new(SDK_VENDOR_DIR).join(relative),
            content: (*content).to_string(),
        })
        .collect()
}

/// Build the Python source for `workflow`, returning a `GeneratedSource`
/// rooted at `<stem>.py` plus the vendored `loopt_sdk` package under
/// `.loopt/sdk/python` that the generated module imports.
pub fn emit(workflow: &Value, stem: &Path) -> Result<GeneratedSource> {
    let mut literal = String::new();
    write_value(&mut literal, workflow, 1)?;

    let entrypoint = stem.with_extension("py");
    let content = render_module(&literal, &sdk_path_from(&entrypoint));

    let mut files = vec![GeneratedFile {
        path: entrypoint.clone(),
        content,
    }];
    files.extend(sdk_files());

    Ok(GeneratedSource { entrypoint, files })
}

/// Project-relative path from the generated module's directory to the
/// vendored SDK directory (`.loopt/sdk/python`).
///
/// Workflow sources live at `.loopt/workflows/<stem>.py` while imported
/// actions keep their parent path at
/// `.loopt/workflows/actions/<...>/<stem>.py`, so the number of `..`
/// components depends on the entrypoint's depth.
fn sdk_path_from(entrypoint: &Path) -> String {
    let directory = entrypoint.parent().unwrap_or(Path::new(""));
    let depth = directory.components().count();
    let workflows_depth = Path::new(".loopt/workflows").components().count();
    let ups = depth.saturating_sub(workflows_depth) + 1;
    format!("{}sdk/python", "../".repeat(ups))
}

/// Describe how to execute a previously generated Python source file.
pub fn command(source: &Path) -> Result<CommandSpec> {
    let path = source
        .to_str()
        .with_context(|| format!("source path {} is not valid UTF-8", source.display()))?
        .to_string();

    Ok(CommandSpec {
        program: "python3".to_string(),
        args: vec![path],
    })
}

/// Wrap the top-level Python literal in an editable module: a
/// `build_workflow()` function returning the literal, and a `main()` that
/// prints it through the vendored SDK as one JSON object on stdout (UTF-8,
/// order-safe because Python dicts preserve insertion order).
fn render_module(literal: &str, sdk_path: &str) -> String {
    format!(
        "{header}\n\ndef build_workflow():\n{indent}return {literal}\n\n\ndef main() -> None:\n\
         {indent}print_workflow(build_workflow())\n\n\nif __name__ == \"__main__\":\n{indent}main()\n",
        header = module_header(sdk_path),
        indent = INDENT,
        literal = literal,
    )
}

/// The module preamble, pointing `sys.path` at `sdk_path` relative to the
/// generated file so the vendored `loopt_sdk` package imports without an
/// installed distribution.
fn module_header(sdk_path: &str) -> String {
    format!(
        "#!/usr/bin/env python3\n\"\"\"Generated by `loopt import`.\n\nEdit the Python value returned by build_workflow() below -- you may add\nimports, helper functions, loops or conditionals as needed -- then run\n`loopt gen` to regenerate the workflow YAML from this source.\n\nThe workflow is printed through the vendored `loopt_sdk` package under\n`.loopt/sdk/python`, so no installation is required.\n\"\"\"\n\nimport os\nimport sys\n\nsys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), \"{sdk_path}\"))\n\nfrom loopt_sdk.emit import print_workflow"
    )
}

/// Recursively render `value` as a Python literal. `indent` is the nesting
/// depth (in INDENT units) of the *contents* of the literal being written;
/// the returned string has no leading indentation of its own so the caller
/// can splice it directly after `return `, `key: `, or inside a list.
fn write_value(out: &mut String, value: &Value, indent: usize) -> Result<()> {
    match value {
        Value::Null => out.push_str("None"),
        Value::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
        Value::Number(n) => out.push_str(&format_number(n)),
        Value::String(s) => out.push_str(&python_str_literal(s)),
        Value::Array(items) => write_array(out, items, indent)?,
        Value::Object(map) => write_object(out, map, indent)?,
    }
    Ok(())
}

fn write_array(out: &mut String, items: &[Value], indent: usize) -> Result<()> {
    if items.is_empty() {
        out.push_str("[]");
        return Ok(());
    }
    let pad = INDENT.repeat(indent);
    let pad_outer = INDENT.repeat(indent.saturating_sub(1));
    out.push_str("[\n");
    for item in items {
        out.push_str(&pad);
        write_value(out, item, indent + 1)?;
        out.push_str(",\n");
    }
    out.push_str(&pad_outer);
    out.push(']');
    Ok(())
}

fn write_object(
    out: &mut String,
    map: &serde_json::Map<String, Value>,
    indent: usize,
) -> Result<()> {
    if map.is_empty() {
        out.push_str("{}");
        return Ok(());
    }
    let pad = INDENT.repeat(indent);
    let pad_outer = INDENT.repeat(indent.saturating_sub(1));
    out.push_str("{\n");
    for (key, val) in map {
        out.push_str(&pad);
        out.push_str(&python_str_literal(key));
        out.push_str(": ");
        write_value(out, val, indent + 1)?;
        out.push_str(",\n");
    }
    out.push_str(&pad_outer);
    out.push('}');
    Ok(())
}

/// Render a JSON number as the Python literal with matching type: integers
/// stay integers (Python's `int` is arbitrary precision, so no precision is
/// lost regardless of magnitude), and floats use the shortest round-trip
/// decimal representation, matching Python's own `repr(float)` algorithm.
fn format_number(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else if let Some(f) = n.as_f64() {
        format_float(f)
    } else {
        // serde_json::Number is always i64, u64, or f64 representable
        // (or, with the `arbitrary_precision` feature, an exact decimal
        // string); fall back to that decimal text verbatim so no
        // precision is silently lost.
        n.to_string()
    }
}

fn format_float(f: f64) -> String {
    // Rust's `{:?}` for f64 produces the shortest decimal string that
    // round-trips back to the same bits, which is exactly what Python's
    // `repr(float)` guarantees since Python 3.1. Rust always includes a
    // decimal point or exponent for non-integral-looking output, but when
    // the value is a whole number ("5") it needs an explicit ".0" suffix
    // to remain a Python float literal rather than an int literal.
    let mut repr = format!("{f:?}");
    if !repr.contains('.') && !repr.contains('e') && !repr.contains('E') {
        repr.push_str(".0");
    }
    repr
}

/// Render `s` as a Python string literal, choosing the quote character the
/// way CPython's `repr()` does (prefer `'`, switch to `"` only when the
/// string contains a `'` but no `"`) and escaping exactly the characters
/// that need it. Printable Unicode is emitted verbatim since generated
/// sources are UTF-8 Python files.
fn python_str_literal(s: &str) -> String {
    let use_double_quote = s.contains('\'') && !s.contains('"');
    let quote = if use_double_quote { '"' } else { '\'' };

    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

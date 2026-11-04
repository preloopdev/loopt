//! Executes a `CommandSpec` (a generated or hand-authored source's runtime)
//! and parses its single JSON object on stdout as a workflow value.
//!
//! This is the stdout protocol every authoring source (built-in or custom
//! plugin-authored) speaks: stdout is exactly one JSON object, stderr is
//! free-form diagnostics shown to the user.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::languages::CommandSpec;

/// Run `command` with `cwd` as the working directory, no shell, and parse
/// its stdout as exactly one JSON object. On any failure the full stdout
/// and stderr are included verbatim (no truncation) so the caller can show
/// the real failure, not a summary.
pub fn run_source(command: &CommandSpec, cwd: &Path) -> Result<serde_json::Value> {
    let rendered = format!("{} {}", command.program, command.args.join(" "));
    let output = Command::new(&command.program)
        .args(&command.args)
        .envs(command.env.iter().map(|(key, value)| (key, value)))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("spawn `{rendered}`"))?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!(
            "`{rendered}` exited with {}\n{}",
            output.status,
            stderr.trim_end()
        );
    }

    let stdout = String::from_utf8(output.stdout)
        .with_context(|| format!("`{rendered}` wrote non-UTF-8 stdout"))?;

    let value: serde_json::Value = serde_json::from_str(&stdout).with_context(|| {
        format!(
            "`{rendered}` did not print one JSON object on stdout:\n{stdout}\nstderr:\n{}",
            stderr.trim_end()
        )
    })?;

    if !value.is_object() {
        bail!(
            "`{rendered}` printed a JSON {}, not an object",
            json_kind(&value)
        );
    }

    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr.trim_end());
    }

    Ok(value)
}

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

//! Script-level checks: shell names and deprecated workflow commands.
//!
//! Shell names mirror:
//! <https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#jobsjob_idstepsshell>
//! Deprecated commands mirror:
//! <https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands>

use serde_json::{Map, Value};

use crate::{Confidence, Diagnostic, Severity};

const BUILTIN_SHELLS: &[&str] = &["bash", "cmd", "powershell", "pwsh", "python", "sh"];

const DEPRECATED_COMMANDS: &[(&str, &str)] = &[
    ("add-path", "$GITHUB_PATH"),
    ("save-state", "$GITHUB_STATE"),
    ("set-env", "$GITHUB_ENV"),
    ("set-output", "$GITHUB_OUTPUT"),
];

pub(crate) fn check(doc: &Value, findings: &mut Vec<Diagnostic>) {
    let Some(root) = doc.as_object() else {
        return;
    };
    if let Some(defaults) = root.get("defaults") {
        check_defaults_shell(defaults, "/defaults", findings);
    }
    let Some(jobs) = root.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        let Some(job) = job.as_object() else {
            continue;
        };
        let path = format!("/jobs/{}", escape(job_id));
        if let Some(defaults) = job.get("defaults") {
            check_defaults_shell(defaults, &format!("{path}/defaults"), findings);
        }
        if let Some(steps) = job.get("steps").and_then(Value::as_array) {
            check_steps(steps, &format!("{path}/steps"), findings);
        }
    }
}

fn check_defaults_shell(defaults: &Value, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(shell) = defaults.get("run").and_then(|run| run.get("shell")) else {
        return;
    };
    check_shell(shell, &format!("{path}/run/shell"), findings);
}

fn check_steps(steps: &[Value], path: &str, findings: &mut Vec<Diagnostic>) {
    for (index, step) in steps.iter().enumerate() {
        let Some(step) = step.as_object() else {
            continue;
        };
        let step_path = format!("{path}/{index}");
        if step.contains_key("run")
            && let Some(shell) = step.get("shell")
        {
            check_shell(shell, &format!("{step_path}/shell"), findings);
        }
        check_deprecated(step, &step_path, findings);
        if let Some(parallel) = step.get("parallel").and_then(Value::as_array) {
            check_steps(parallel, &format!("{step_path}/parallel"), findings);
        }
    }
}

fn check_shell(value: &Value, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(shell) = value.as_str() else {
        return;
    };
    let shell = shell.trim();
    // A custom shell is a command template, and expressions are unknown.
    if shell.is_empty() || shell.contains("${{") || shell.contains("{0}") {
        return;
    }
    if BUILTIN_SHELLS
        .iter()
        .any(|known| shell.eq_ignore_ascii_case(known))
    {
        return;
    }
    findings.push(error(
        "preloop/invalid-shell",
        path,
        format!("`{shell}` is not a GitHub Actions shell name"),
        "use bash, pwsh, python, sh, cmd, or powershell, or a custom command template containing `{0}`",
    ));
}

fn check_deprecated(step: &Map<String, Value>, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(script) = step.get("run").and_then(Value::as_str) else {
        return;
    };
    for (command, replacement) in deprecated_commands(script) {
        findings.push(crate::diagnostic(
            "preloop/deprecated-command",
            &format!("{path}/run"),
            format!("`::{command}` is a deprecated GitHub Actions workflow command"),
            &format!("write to `{replacement}` instead of emitting `::{command}`"),
        ));
    }
}

/// Find deprecated command names in a script. Matching is case-insensitive and
/// follows actionlint's command shape: `::set-output name=...` and
/// `::add-path::...`, so prose that merely names a command is not reported.
fn deprecated_commands(script: &str) -> Vec<(&'static str, &'static str)> {
    let bytes = script.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while let Some(offset) = script[index..].find("::") {
        let start = index + offset + 2;
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-') {
            end += 1;
        }
        let name = script[start..end].to_ascii_lowercase();
        let rest = &script[end..];
        let entry = DEPRECATED_COMMANDS
            .iter()
            .find(|(command, _)| *command == name)
            .copied();
        if let Some((command, replacement)) = entry
            && command_matches(command, rest)
        {
            found.push((command, replacement));
        }
        index = end;
    }
    found
}

fn command_matches(command: &str, rest: &str) -> bool {
    if command == "add-path" {
        return rest.starts_with("::");
    }
    rest.chars()
        .next()
        .is_some_and(|character| character.is_ascii_whitespace())
        && rest.trim_start().starts_with("name=")
}

fn error(rule_id: &str, path: &str, message: impl Into<String>, help: &str) -> Diagnostic {
    let mut finding = crate::diagnostic(rule_id, path, message.into(), help);
    finding.severity = Severity::Error;
    finding.confidence = Confidence::High;
    finding
}

fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

//! Actionable shell checking for workflow `run:` scripts.
//!
//! `shellcheck` is GPL-3 software, so it is never bundled or linked: this
//! module only spawns a locally installed executable and translates its JSON
//! findings into portable [`Diagnostic`]s. When the executable is missing,
//! [`ShellcheckMode::Auto`] disables the integration silently, while
//! [`ShellcheckMode::On`] reports that the check could not run.

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;

use anyhow::{bail, Context, Result};
use preloop_gha_authoring::WorkflowDocument;
use preloop_gha_lint::{Confidence, Diagnostic, Severity};
use serde::Deserialize;
use serde_json::Value;

/// Executable name used when no explicit path is configured.
const DEFAULT_EXECUTABLE: &str = "shellcheck";

/// Checks excluded because they need repository evidence, generated workflow
/// values, or literal string data that a `run:` script does not contain.
const EXCLUDED_CHECKS: &str = "SC1091,SC2050,SC2194,SC2154,SC2157,SC2043";

/// Whether `run:` scripts are checked with the external `shellcheck`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellcheckMode {
    /// Use shellcheck when it can be run, silently skip it otherwise.
    Auto,
    /// Require shellcheck; fail when it cannot be run.
    On,
    /// Do not run shellcheck.
    Off,
}

impl ShellcheckMode {
    /// Parse `auto`, `on`, or `off`, rejecting anything else.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "on" => Ok(Self::On),
            "off" => Ok(Self::Off),
            other => bail!("unknown shellcheck mode {other:?}; expected auto, on, or off"),
        }
    }
}

/// Configuration for the shellcheck integration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellcheckOptions {
    /// Whether shellcheck is optional, required, or disabled.
    pub mode: ShellcheckMode,
    /// Executable path or name; defaults to `shellcheck` on `PATH`.
    pub path: Option<String>,
}

impl Default for ShellcheckOptions {
    fn default() -> Self {
        Self {
            mode: ShellcheckMode::Auto,
            path: None,
        }
    }
}

/// Check every eligible `run:` script in `document` and return one
/// [`Diagnostic`] per shellcheck finding.
///
/// Findings stay in shellcheck's order; callers sort and deduplicate them
/// together with their other checks. An unavailable executable yields no
/// findings in [`ShellcheckMode::Auto`] and an error in
/// [`ShellcheckMode::On`].
pub fn check(document: &WorkflowDocument, options: &ShellcheckOptions) -> Result<Vec<Diagnostic>> {
    if options.mode == ShellcheckMode::Off {
        return Ok(Vec::new());
    }
    let executable = options.path.as_deref().unwrap_or(DEFAULT_EXECUTABLE);
    if !available(executable, options.mode)? {
        return Ok(Vec::new());
    }

    let scripts = extract_scripts(document.value());
    let mut diagnostics = Vec::new();
    for script in scripts {
        if script.text.trim().is_empty() {
            continue;
        }
        let Some(shell) = script.shell.and_then(shell_argument) else {
            continue;
        };
        let masked = mask_expressions(script.text);
        let path = &script.path;
        let findings = run_shellcheck(executable, shell, masked, path)
            .map_err(|error| error.context(format!("check {path}")))?;
        diagnostics.extend(findings);
    }
    Ok(diagnostics)
}

/// Probe `<executable> --version`. Returns `false` only when the integration
/// is optional and the executable cannot be used.
fn available(executable: &str, mode: ShellcheckMode) -> Result<bool> {
    match Command::new(executable).arg("--version").output() {
        Ok(output) if output.status.success() => Ok(true),
        Ok(output) => {
            if mode == ShellcheckMode::On {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let detail = stderr.trim();
                let detail = if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                };
                bail!(
                    "shellcheck could not be run: `{executable} --version` exited with {}{detail}; install shellcheck or pass --shellcheck-path",
                    output.status
                );
            }
            Ok(false)
        }
        Err(error) => {
            if mode == ShellcheckMode::On {
                bail!(
                    "shellcheck could not be run: {error}; install shellcheck or pass --shellcheck-path"
                );
            }
            Ok(false)
        }
    }
}

/// Run shellcheck over one masked script and translate its JSON findings.
fn run_shellcheck(
    executable: &str,
    shell: &str,
    script: String,
    path: &str,
) -> Result<Vec<Diagnostic>> {
    let mut child = Command::new(executable)
        .arg("--format=json")
        .arg(format!("--shell={shell}"))
        .arg(format!("--exclude={EXCLUDED_CHECKS}"))
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("run {executable}"))?;

    // Feed stdin from a thread so a large script cannot deadlock against the
    // stdout/stderr pipes read by `wait_with_output`.
    let mut stdin = child.stdin.take().context("open shellcheck stdin")?;
    let writer = thread::spawn(move || {
        // shellcheck may exit before consuming everything; its verdict is the
        // exit status, so a broken pipe here is not itself an error.
        let _ = stdin.write_all(script.as_bytes());
    });
    let output = child.wait_with_output().context("wait for shellcheck")?;
    let _ = writer.join();

    if !matches!(output.status.code(), Some(0 | 1)) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "shellcheck failed with {}: {}",
            output.status,
            stderr.trim()
        );
    }
    parse_findings(&output.stdout, path)
}

/// Shellcheck finding as emitted by `--format=json`. Fields this integration
/// does not use are ignored during deserialization.
#[derive(Deserialize)]
struct ShellcheckFinding {
    code: u32,
    level: String,
    line: usize,
    column: usize,
    message: String,
}

/// Parse shellcheck's JSON array into diagnostics rooted at `path`.
fn parse_findings(stdout: &[u8], path: &str) -> Result<Vec<Diagnostic>> {
    let findings: Vec<ShellcheckFinding> =
        serde_json::from_slice(stdout).context("parse shellcheck JSON output")?;
    Ok(findings
        .into_iter()
        .map(|finding| into_diagnostic(finding, path))
        .collect())
}

/// Map one shellcheck finding to a portable diagnostic.
fn into_diagnostic(finding: ShellcheckFinding, path: &str) -> Diagnostic {
    let ShellcheckFinding {
        code,
        level,
        line,
        column,
        message,
    } = finding;
    Diagnostic {
        rule_id: format!("shellcheck/SC{code}"),
        severity: severity(&level),
        confidence: Confidence::High,
        path: path.to_owned(),
        message: format!("{message} (script line {line}, column {column})"),
        help: Some(format!("https://www.shellcheck.net/wiki/SC{code}")),
        line: None,
        column: None,
        source: "shellcheck".to_owned(),
    }
}

/// Map shellcheck's level names to diagnostic severities.
fn severity(level: &str) -> Severity {
    match level {
        "error" => Severity::Error,
        "warning" => Severity::Warning,
        _ => Severity::Info,
    }
}

/// One `run:` script selected for checking.
struct Script<'a> {
    text: &'a str,
    shell: Option<&'a str>,
    path: String,
}

/// Collect script steps from a workflow (`jobs`) or an action (`runs`).
fn extract_scripts(value: &Value) -> Vec<Script<'_>> {
    let mut scripts = Vec::new();
    let workflow_shell = run_default_shell(value);

    if let Some(jobs) = value.get("jobs").and_then(Value::as_object) {
        for (id, job) in jobs {
            if job.get("uses").is_some() {
                continue;
            }
            let job_shell = run_default_shell(job);
            let runs_on = job.get("runs-on");
            let Some(steps) = job.get("steps").and_then(Value::as_array) else {
                continue;
            };
            for (index, step) in steps.iter().enumerate() {
                let Some(text) = step.get("run").and_then(Value::as_str) else {
                    continue;
                };
                let shell = resolve_shell(
                    step.get("shell").and_then(Value::as_str),
                    job_shell,
                    workflow_shell,
                    runs_on,
                );
                scripts.push(Script {
                    text,
                    shell: Some(shell),
                    path: format!("/jobs/{}/steps/{index}/run", escape_pointer_token(id)),
                });
            }
        }
    }

    if let Some(steps) = value
        .get("runs")
        .and_then(|runs| runs.get("steps"))
        .and_then(Value::as_array)
    {
        for (index, step) in steps.iter().enumerate() {
            let Some(text) = step.get("run").and_then(Value::as_str) else {
                continue;
            };
            scripts.push(Script {
                text,
                shell: Some(resolve_shell(
                    step.get("shell").and_then(Value::as_str),
                    None,
                    None,
                    None,
                )),
                path: format!("/runs/steps/{index}/run"),
            });
        }
    }

    scripts
}

/// `defaults.run.shell` for a workflow or job mapping.
fn run_default_shell(value: &Value) -> Option<&str> {
    value
        .get("defaults")
        .and_then(|defaults| defaults.get("run"))
        .and_then(|run| run.get("shell"))
        .and_then(Value::as_str)
}

/// Pick the shell for one step: step override, job default, workflow default,
/// then the runner's default shell.
fn resolve_shell<'a>(
    step: Option<&'a str>,
    job: Option<&'a str>,
    workflow: Option<&'a str>,
    runs_on: Option<&Value>,
) -> &'a str {
    step.or(job)
        .or(workflow)
        .unwrap_or_else(|| default_shell(runs_on))
}

/// GitHub's default shell for a runner: PowerShell on Windows, bash elsewhere.
fn default_shell(runs_on: Option<&Value>) -> &'static str {
    if runs_on.is_some_and(has_windows_label) {
        "pwsh"
    } else {
        "bash"
    }
}

/// Whether any runner label names a Windows image.
fn has_windows_label(runs_on: &Value) -> bool {
    runner_labels(runs_on).into_iter().any(|label| {
        label
            .get(.."windows".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("windows"))
    })
}

/// Runner labels from a string, an array of strings, or an object with a
/// `labels` string or array. Non-string entries are ignored.
fn runner_labels(runs_on: &Value) -> Vec<&str> {
    let mut labels = Vec::new();
    match runs_on {
        Value::String(label) => labels.push(label.as_str()),
        Value::Array(entries) => labels.extend(entries.iter().filter_map(Value::as_str)),
        Value::Object(map) => {
            if let Some(declared) = map.get("labels") {
                match declared {
                    Value::String(label) => labels.push(label.as_str()),
                    Value::Array(entries) => {
                        labels.extend(entries.iter().filter_map(Value::as_str))
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    labels
}

/// Shellcheck's `--shell` value when the configured shell names bash or sh.
fn shell_argument(shell: &str) -> Option<&'static str> {
    if shell.contains("${{") {
        return None;
    }
    match shell.split_whitespace().next()? {
        "bash" => Some("bash"),
        "sh" => Some("sh"),
        _ => None,
    }
}

/// Replace every `${{ ... }}` span with underscores, preserving newline
/// characters and the total character count so shellcheck line numbers still
/// match the workflow source. An unterminated expression is left unchanged.
fn mask_expressions(script: &str) -> String {
    let mut masked = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${{") {
        masked.push_str(&rest[..start]);
        let after_open = start + "${{".len();
        let Some(end) = expression_end(rest, after_open) else {
            masked.push_str(&rest[start..]);
            return masked;
        };
        masked.extend(rest[start..end].chars().map(|character| {
            if matches!(character, '\n' | '\r') {
                character
            } else {
                '_'
            }
        }));
        rest = &rest[end..];
    }
    masked.push_str(rest);
    masked
}

/// Byte index just past the closing `}}` of an expression whose body starts
/// at `index`. `}}` inside single-quoted literals is ignored; `''` is an
/// escaped quote inside such a literal.
fn expression_end(input: &str, mut index: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut in_single = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_single {
            if byte == b'\'' {
                if bytes.get(index + 1) == Some(&b'\'') {
                    index += 2;
                } else {
                    in_single = false;
                    index += 1;
                }
            } else {
                index += 1;
            }
        } else if byte == b'\'' {
            in_single = true;
            index += 1;
        } else if byte == b'}' && bytes.get(index + 1) == Some(&b'}') {
            return Some(index + 2);
        } else {
            index += 1;
        }
    }
    None
}

/// Escape one RFC 6901 JSON Pointer token.
fn escape_pointer_token(token: &str) -> String {
    let mut escaped = String::with_capacity(token.len());
    for character in token.chars() {
        match character {
            '~' => escaped.push_str("~0"),
            '/' => escaped.push_str("~1"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shellcheck_mode_parsing() {
        assert_eq!(ShellcheckMode::parse("auto").unwrap(), ShellcheckMode::Auto);
        assert_eq!(ShellcheckMode::parse("on").unwrap(), ShellcheckMode::On);
        assert_eq!(ShellcheckMode::parse("off").unwrap(), ShellcheckMode::Off);
        let error = ShellcheckMode::parse("sometimes").unwrap_err().to_string();
        assert!(error.contains("auto"), "{error}");
        assert!(error.contains("on"), "{error}");
        assert!(error.contains("off"), "{error}");
    }

    #[test]
    fn masking_keeps_length_and_newlines() {
        let script = "echo ${{ inputs.name }}\necho ${{ matrix.os }}\n";
        let masked = mask_expressions(script);
        assert_eq!(masked.chars().count(), script.chars().count());
        assert_eq!(masked.matches('\n').count(), script.matches('\n').count());
        assert_eq!(
            masked,
            format!("echo {}\necho {}\n", "_".repeat(18), "_".repeat(16))
        );
    }

    #[test]
    fn masking_ignores_braces_inside_single_quoted_literals() {
        let script = "run ${{ 'a}}b' }} tail";
        let masked = mask_expressions(script);
        assert_eq!(masked, format!("run {} tail", "_".repeat(13)));

        let escaped = "run ${{ 'it''s }} here' }} tail";
        let masked = mask_expressions(escaped);
        let span = "${{ 'it''s }} here' }}";
        assert_eq!(
            masked,
            format!("run {} tail", "_".repeat(span.chars().count()))
        );
    }

    #[test]
    fn masking_leaves_unterminated_expressions_alone() {
        assert_eq!(
            mask_expressions("echo ${{ unterminated"),
            "echo ${{ unterminated"
        );
    }

    #[test]
    fn shell_arguments_only_cover_bash_and_sh() {
        assert_eq!(shell_argument("bash"), Some("bash"));
        assert_eq!(shell_argument("sh"), Some("sh"));
        assert_eq!(shell_argument("bash -e {0}"), Some("bash"));
        assert_eq!(
            shell_argument("bash --noprofile --norc -eo pipefail {0}"),
            Some("bash")
        );
        assert_eq!(shell_argument("sh -ec {0}"), Some("sh"));

        assert_eq!(shell_argument("pwsh"), None);
        assert_eq!(shell_argument("powershell -command . '{0}'"), None);
        assert_eq!(shell_argument("python"), None);
        assert_eq!(shell_argument("cmd /c {0}"), None);
        assert_eq!(shell_argument("/usr/bin/env bash"), None);
        assert_eq!(shell_argument("${{ inputs.shell }}"), None);
    }

    #[test]
    fn shell_resolution_prefers_step_then_job_then_workflow() {
        assert_eq!(
            resolve_shell(Some("bash -e {0}"), Some("sh"), Some("sh"), None),
            "bash -e {0}"
        );
        assert_eq!(resolve_shell(None, Some("sh"), Some("bash"), None), "sh");
        assert_eq!(resolve_shell(None, None, Some("sh"), None), "sh");
        assert_eq!(resolve_shell(None, None, None, None), "bash");
    }

    #[test]
    fn windows_runners_default_to_pwsh() {
        let windows = json!("windows-latest");
        let windows_labels = json!({"labels": ["self-hosted", "Windows", "x64"]});
        let windows_label = json!({"labels": "windows-2022"});
        let linux = json!(["ubuntu-latest"]);
        let mixed = json!(["ubuntu-latest", 42]);

        assert_eq!(default_shell(Some(&windows)), "pwsh");
        assert_eq!(default_shell(Some(&windows_labels)), "pwsh");
        assert_eq!(default_shell(Some(&windows_label)), "pwsh");
        assert_eq!(default_shell(Some(&linux)), "bash");
        assert_eq!(default_shell(Some(&mixed)), "bash");
        assert_eq!(default_shell(None), "bash");

        assert_eq!(
            shell_argument(resolve_shell(None, None, None, Some(&windows))),
            None
        );
        assert_eq!(
            resolve_shell(Some("bash"), None, None, Some(&windows)),
            "bash"
        );
        assert_eq!(
            shell_argument(resolve_shell(Some("bash"), None, None, Some(&windows))),
            Some("bash")
        );
    }

    #[test]
    fn severity_levels_match_shellcheck() {
        assert_eq!(severity("error"), Severity::Error);
        assert_eq!(severity("warning"), Severity::Warning);
        assert_eq!(severity("info"), Severity::Info);
        assert_eq!(severity("style"), Severity::Info);
    }

    #[test]
    fn pointer_tokens_escape_tilde_and_slash() {
        assert_eq!(escape_pointer_token("build"), "build");
        assert_eq!(escape_pointer_token("a/b~c"), "a~1b~0c");
        assert_eq!(escape_pointer_token("~1/"), "~01~1");
    }

    #[test]
    fn extraction_skips_reusable_jobs_and_non_string_runs() {
        let workflow = json!({
            "defaults": {"run": {"shell": "sh"}},
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        {"run": "echo one"},
                        {"run": "echo two", "shell": "bash -e {0}"},
                        {"uses": "actions/checkout@v4"},
                        {"run": 7}
                    ]
                },
                "weird/id~": {
                    "runs-on": "ubuntu-latest",
                    "steps": [{"run": "echo three"}]
                },
                "reuse": {"uses": "octo/repo/.github/workflows/reuse.yml"},
                "none": {}
            }
        });

        let scripts = extract_scripts(&workflow);
        assert_eq!(scripts.len(), 3);
        assert_eq!(scripts[0].text, "echo one");
        assert_eq!(scripts[0].shell, Some("sh"));
        assert_eq!(scripts[0].path, "/jobs/build/steps/0/run");
        assert_eq!(scripts[1].shell, Some("bash -e {0}"));
        assert_eq!(scripts[1].path, "/jobs/build/steps/1/run");
        assert_eq!(scripts[2].path, "/jobs/weird~1id~0/steps/0/run");
    }

    #[test]
    fn extraction_reads_action_steps() {
        let action = json!({
            "runs": {
                "using": "composite",
                "steps": [
                    {"run": "echo action", "shell": "bash"},
                    {"run": "echo default"},
                    {"uses": "actions/checkout@v4"}
                ]
            }
        });

        let scripts = extract_scripts(&action);
        assert_eq!(scripts.len(), 2);
        assert_eq!(scripts[0].path, "/runs/steps/0/run");
        assert_eq!(scripts[0].shell, Some("bash"));
        assert_eq!(scripts[1].path, "/runs/steps/1/run");
        assert_eq!(scripts[1].shell, Some("bash"));
    }

    #[test]
    fn maps_shellcheck_json_to_diagnostics() {
        let stdout = r#"[
            {"file":"-","line":3,"endLine":3,"column":5,"endColumn":8,"level":"warning","code":2086,"message":"Double quote to prevent globbing and word splitting.","fix":null},
            {"file":"-","line":1,"endLine":1,"column":1,"endColumn":2,"level":"error","code":1091,"message":"Not following: file","fix":null},
            {"file":"-","line":2,"endLine":2,"column":1,"endColumn":2,"level":"style","code":2043,"message":"This loop will only ever run once.","fix":null},
            {"file":"-","line":4,"endLine":4,"column":1,"endColumn":2,"level":"info","code":2154,"message":"var is referenced but not assigned.","fix":null}
        ]"#;

        let diagnostics = parse_findings(stdout.as_bytes(), "/jobs/build/steps/0/run").unwrap();
        assert_eq!(diagnostics.len(), 4);

        let first = &diagnostics[0];
        assert_eq!(first.rule_id, "shellcheck/SC2086");
        assert_eq!(first.severity, Severity::Warning);
        assert_eq!(first.confidence, Confidence::High);
        assert_eq!(first.path, "/jobs/build/steps/0/run");
        assert_eq!(
            first.message,
            "Double quote to prevent globbing and word splitting. (script line 3, column 5)"
        );
        assert_eq!(
            first.help.as_deref(),
            Some("https://www.shellcheck.net/wiki/SC2086")
        );
        assert_eq!(first.line, None);
        assert_eq!(first.column, None);
        assert_eq!(first.source, "shellcheck");

        assert_eq!(diagnostics[1].rule_id, "shellcheck/SC1091");
        assert_eq!(diagnostics[1].severity, Severity::Error);
        assert_eq!(diagnostics[2].severity, Severity::Info);
        assert_eq!(diagnostics[3].severity, Severity::Info);
    }
}

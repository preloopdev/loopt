//! Small, offline authoring checks for the current Preloop execution context.
//!
//! Context checks use explicitly supplied inventories; absent inventories mean
//! unknown, not empty. Findings are advisories, not a workflow schema gate.
//! The core never starts subprocesses or uses a network.

use std::collections::{BTreeMap, BTreeSet};

use preloop_gha_authoring::WorkflowDocument;
use preloop_gha_expressions::{collect_secret_reads, validate_expression};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod context;
mod structure;
mod triggers;

/// Importance of a finding, independently of confidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// A definite error reported by a linter.
    Error,
    /// A condition worth reviewing before execution.
    Warning,
    /// Advisory information, including statically unknown behavior.
    Info,
}

/// Certainty of the observation, not a proof of exploitability.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Directly supported by the document and supplied context.
    High,
    /// Execution-time information is needed to resolve the observation.
    Medium,
}

/// A portable authoring finding. Messages never include secret values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Stable identifier, namespaced by the producing linter.
    pub rule_id: String,
    /// Importance of the finding.
    pub severity: Severity,
    /// Certainty of the observation.
    pub confidence: Confidence,
    /// RFC 6901 JSON Pointer; empty if only a source location is known.
    pub path: String,
    /// Human-readable observation, without reproducing expression contents.
    pub message: String,
    /// Suggested action or explanation of uncertainty.
    pub help: Option<String>,
    /// One-based source line, when supplied by the producer.
    pub line: Option<usize>,
    /// One-based source column, when supplied by the producer.
    pub column: Option<usize>,
    /// Producer: `preloop` or `jactionlint`.
    pub source: String,
}

/// Optional execution inventories. `None` means unknown; `Some(empty)` means
/// known empty. Secret names and runner labels are compared case-insensitively;
/// environment names retain Preloop's case-sensitive identity.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LintContext {
    /// Label sets of runners available to execute the workflow.
    #[serde(default)]
    pub runner_label_sets: Option<Vec<BTreeSet<String>>>,
    /// Workflow-wide secret names; never their values.
    #[serde(default)]
    pub available_secrets: Option<BTreeSet<String>>,
    /// Known secret-name inventories per environment. Missing entries are unknown.
    #[serde(default)]
    pub environment_secrets: BTreeMap<String, BTreeSet<String>>,
    /// Registered environment names, if known.
    #[serde(default)]
    pub known_environments: Option<BTreeSet<String>>,
}

/// Inspect an unexpanded document without enforcing an execution schema.
///
/// The expression engine supplies syntax/function validation and secret-read
/// analysis. A dynamic secret read is uncertainty, not evidence of leakage.
/// Literal disabled jobs/steps retain syntax checks but omit execution advisories.
/// Source coordinates are left unknown rather than guessed from YAML text.
/// Results are ordered by JSON Pointer, rule, producer, then message.
pub fn lint_workflow(document: &WorkflowDocument, context: &LintContext) -> Vec<Diagnostic> {
    let mut findings = Vec::new();
    walk(document.value(), "", Scope::Root, false, &mut findings);
    structure::check(document.value(), &mut findings);
    triggers::check(document.value(), &mut findings);
    context::check(document, context, &mut findings);
    findings.sort_by(|left, right| {
        (&left.path, &left.rule_id, &left.source, &left.message).cmp(&(
            &right.path,
            &right.rule_id,
            &right.source,
            &right.message,
        ))
    });
    findings.dedup();
    findings
}

pub(crate) fn diagnostic(rule_id: &str, path: &str, message: String, help: &str) -> Diagnostic {
    Diagnostic {
        rule_id: rule_id.to_owned(),
        severity: Severity::Warning,
        confidence: Confidence::High,
        path: path.to_owned(),
        message,
        help: Some(help.to_owned()),
        line: None,
        column: None,
        source: "preloop".to_owned(),
    }
}

/// Extract complete expression bodies, ignoring braces inside single-quoted
/// expression strings and respecting the expression language's doubled quotes.
pub(crate) fn expressions(text: &str) -> Vec<&str> {
    expression_parts(text)
        .filter_map(|(body, complete)| complete.then_some(body))
        .collect()
}

fn expression_parts(text: &str) -> impl Iterator<Item = (&str, bool)> {
    let mut cursor = 0;
    std::iter::from_fn(move || {
        let relative_start = text[cursor..].find("${{")?;
        let start = cursor + relative_start + 3;
        let bytes = text.as_bytes();
        let mut index = start;
        let mut quoted = false;
        while index < bytes.len() {
            if bytes[index] == b'\'' {
                if quoted && bytes.get(index + 1) == Some(&b'\'') {
                    index += 2;
                    continue;
                }
                quoted = !quoted;
            } else if !quoted && bytes[index] == b'}' && bytes.get(index + 1) == Some(&b'}') {
                cursor = index + 2;
                return Some((text[start..index].trim(), true));
            }
            index += 1;
        }
        cursor = text.len();
        Some((text[start..].trim(), false))
    })
}

/// Only a literal false condition disables static execution advisories. No
/// expression evaluation occurs, even for otherwise constant-looking functions.
pub(crate) fn is_disabled(value: &Value) -> bool {
    match value.get("if") {
        Some(Value::Bool(false)) => true,
        Some(Value::String(text)) => {
            preloop_gha_expressions::trim_expression_markers(text).eq_ignore_ascii_case("false")
        }
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum Scope {
    Root,
    Jobs,
    Job,
    Runs,
    Steps,
    Step,
    Generic,
}

fn walk(value: &Value, path: &str, scope: Scope, disabled: bool, findings: &mut Vec<Diagnostic>) {
    let disabled = disabled || (matches!(scope, Scope::Job | Scope::Step) && is_disabled(value));
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let child_path = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                if key == "if" && matches!(scope, Scope::Job | Scope::Step) {
                    if let Value::String(text) = child {
                        check_text(text, &child_path, true, disabled, findings);
                    }
                    continue;
                }
                let child_scope = match (scope, key.as_str()) {
                    (Scope::Root, "jobs") => Scope::Jobs,
                    (Scope::Jobs, _) => Scope::Job,
                    (Scope::Root, "runs") => Scope::Runs,
                    (Scope::Job | Scope::Runs, "steps") => Scope::Steps,
                    _ => Scope::Generic,
                };
                walk(child, &child_path, child_scope, disabled, findings);
            }
        }
        Value::Array(array) => {
            for (index, child) in array.iter().enumerate() {
                let child_scope = if matches!(scope, Scope::Steps) {
                    Scope::Step
                } else {
                    Scope::Generic
                };
                walk(
                    child,
                    &format!("{path}/{index}"),
                    child_scope,
                    disabled,
                    findings,
                );
            }
        }
        Value::String(text) => check_text(text, path, false, disabled, findings),
        _ => {}
    }
}

fn check_text(
    text: &str,
    path: &str,
    condition: bool,
    disabled: bool,
    findings: &mut Vec<Diagnostic>,
) {
    if condition && !text.contains("${{") {
        check_expression(text, path, disabled, findings);
        return;
    }
    for (body, complete) in expression_parts(text) {
        if complete {
            check_expression(body, path, disabled, findings);
        } else {
            findings.push(diagnostic(
                "preloop/expression-syntax",
                path,
                "Expression has unclosed delimiters or an unterminated string.".to_owned(),
                "Close the expression and any single-quoted strings; doubled single quotes escape a quote.",
            ));
        }
    }
}

fn check_expression(expression: &str, path: &str, disabled: bool, findings: &mut Vec<Diagnostic>) {
    if validate_expression(expression).is_err() {
        findings.push(diagnostic(
            "preloop/expression-syntax",
            path,
            "Expression syntax or function signature is not supported by the Preloop expression engine.".to_owned(),
            "Review the expression with a full workflow linter; this check does not evaluate it or enforce a workflow schema.",
        ));
        return;
    }
    if !disabled && collect_secret_reads(expression).is_ok_and(|reads| reads.dynamic) {
        let mut finding = diagnostic(
            "preloop/dynamic-secret-scope",
            path,
            "The secret names read by this expression cannot be determined statically.".to_owned(),
            "Preloop may need every secret in scope to evaluate this expression. Review that scope; this advisory does not establish leakage or a missing secret.",
        );
        finding.severity = Severity::Info;
        finding.confidence = Confidence::Medium;
        findings.push(finding);
    }
}

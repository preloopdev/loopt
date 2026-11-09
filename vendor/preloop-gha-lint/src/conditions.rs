//! Constant-condition checks for `if:`.
//!
//! Mirrors actionlint's `if-cond` rule, including its handling of text around
//! `${{ }}`: GitHub treats such a condition as a non-empty string, so it is
//! always true.
//! <https://github.com/rhysd/actionlint/blob/main/docs/checks.md#if-cond-constant>

use preloop_gha_expressions::{
    Context, collect_contexts, eval_expression, trim_expression_markers,
};
use serde_json::Value;

use crate::{Diagnostic, diagnostic};

pub(crate) fn check(doc: &Value, findings: &mut Vec<Diagnostic>) {
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        let Some(job) = job.as_object() else {
            continue;
        };
        let path = format!("/jobs/{}", escape(job_id));
        check_if(job.get("if"), &format!("{path}/if"), findings);
        if let Some(snapshot) = job.get("snapshot").and_then(Value::as_object) {
            check_if(snapshot.get("if"), &format!("{path}/snapshot/if"), findings);
        }
        if let Some(steps) = job.get("steps").and_then(Value::as_array) {
            check_steps(steps, &format!("{path}/steps"), findings);
        }
    }
}

fn check_steps(steps: &[Value], path: &str, findings: &mut Vec<Diagnostic>) {
    for (index, step) in steps.iter().enumerate() {
        let Some(step) = step.as_object() else {
            continue;
        };
        let step_path = format!("{path}/{index}");
        check_if(step.get("if"), &format!("{step_path}/if"), findings);
        if let Some(parallel) = step.get("parallel").and_then(Value::as_array) {
            check_steps(parallel, &format!("{step_path}/parallel"), findings);
        }
    }
}

fn check_if(value: Option<&Value>, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::Bool(condition) => findings.push(diagnostic(
            "preloop/constant-condition",
            path,
            format!("`if:` is the constant condition `{condition}`"),
            "remove the `if:` field, or replace the literal with a condition that depends on the workflow context",
        )),
        Value::String(text) => check_condition(text, path, findings),
        _ => {}
    }
}

fn check_condition(text: &str, path: &str, findings: &mut Vec<Diagnostic>) {
    let start = text.find("${{");
    let end = text.find("}}");
    match (start, end) {
        (Some(start), Some(end)) => {
            // GitHub only evaluates the `${{ }}` value as a boolean when it is
            // the entire condition. Any surrounding text makes the condition a
            // non-empty string, which is always truthy.
            if start > 0 || end + 2 < text.len() || text.matches("${{").count() > 1 {
                findings.push(diagnostic(
                    "preloop/constant-condition",
                    path,
                    "`if:` is always true because text surrounds the `${{ }}` expression"
                        .to_owned(),
                    "leave only one `${{ }}` expression in the condition, or remove the `if:` field",
                ));
                return;
            }
            let expression = &text[start + 3..end];
            if is_constant(expression) {
                report(path, expression.trim(), findings);
            }
        }
        _ => {
            let expression = trim_expression_markers(text);
            if is_constant(expression) {
                report(path, expression, findings);
            }
        }
    }
}

/// A condition is constant when it references no context (status functions and
/// `hashFiles` count as context) and evaluates without any supplied values.
fn is_constant(expression: &str) -> bool {
    let expression = trim_expression_markers(expression);
    if expression.is_empty() {
        return false;
    }
    let Ok(contexts) = collect_contexts(expression) else {
        return false;
    };
    if !contexts.is_empty() {
        return false;
    }
    eval_expression(expression, &Context::new()).is_ok()
}

fn report(path: &str, expression: &str, findings: &mut Vec<Diagnostic>) {
    findings.push(diagnostic(
        "preloop/constant-condition",
        path,
        format!("`if:` condition `{expression}` is a constant expression"),
        "remove the `if:` field, or replace the literal with a condition that depends on the workflow context",
    ));
}

fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

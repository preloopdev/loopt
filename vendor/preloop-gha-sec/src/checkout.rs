//! `dangerous-checkout`: flags `actions/checkout` steps that turn off the
//! action's fork-checkout protection.
//!
//! Starting with v7, `actions/checkout` refuses to check out fork-originated
//! code on `pull_request_target` and `workflow_run` unless the step explicitly
//! sets `with.allow-unsafe-pr-checkout`. This audit flags that opt-out. It is
//! an independent reimplementation of zizmor's `dangerous-checkout` audit
//! (upstream labels its findings `unsafe-checkout`), including the cases it
//! deliberately ignores:
//!
//! - only literal `actions/checkout@…` steps count; subpath references
//!   (`actions/checkout/some-dir@…`) and reusable-workflow jobs do not;
//! - an absent input, a literal `false`, and a step or job disabled with
//!   `if: false` are clean;
//! - a literal `true` — or any other non-`false` literal, which the action
//!   coerces truthy — is a definite finding;
//! - a `${{ }}` expression in the input, or an opaque `with: ${{ … }}`
//!   mapping, is a lower-confidence finding because the value is unknown at
//!   lint time.
//!
//! See <https://docs.zizmor.sh/audits/#dangerous-checkout>.

use serde_json::Value;

use crate::{AuditContext, Confidence, Diagnostic, Severity};

const RULE: &str = "dangerous-checkout";
const INPUT: &str = "allow-unsafe-pr-checkout";

pub(crate) fn check(doc: &Value, _ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };

    for (job_id, job) in jobs {
        if is_disabled(job) {
            continue;
        }
        let Some(steps) = job.get("steps").and_then(Value::as_array) else {
            continue;
        };
        let job_pointer = crate::pointer("/jobs", job_id);
        let steps_pointer = crate::pointer(&job_pointer, "steps");
        for (index, step) in steps.iter().enumerate() {
            if is_disabled(step) {
                continue;
            }
            let Some(Value::String(uses)) = step.get("uses") else {
                continue;
            };
            if !is_actions_checkout(uses) {
                continue;
            }
            let step_pointer = crate::pointer(&steps_pointer, &index.to_string());
            check_step(step, &step_pointer, out);
        }
    }
}

/// True only for a literal `actions/checkout@<ref>` step reference. The
/// comparison is case-insensitive (GitHub treats owner/repo names that way)
/// and rejects subpaths, matching zizmor's `ExactRepo` coordinate.
fn is_actions_checkout(uses: &str) -> bool {
    if uses.starts_with("./") || uses.starts_with("../") || uses.starts_with("docker://") {
        return false;
    }
    let Some((path, git_ref)) = uses.rsplit_once('@') else {
        return false;
    };
    if path.is_empty() || git_ref.is_empty() {
        return false;
    }
    let mut components = path.split('/');
    let (Some(owner), Some(repo), None) = (components.next(), components.next(), components.next())
    else {
        return false;
    };
    owner.eq_ignore_ascii_case("actions") && repo.eq_ignore_ascii_case("checkout")
}

fn check_step(step: &Value, step_pointer: &str, out: &mut Vec<Diagnostic>) {
    let with_pointer = crate::pointer(step_pointer, "with");
    match step.get("with") {
        None | Some(Value::Null) => {}
        Some(Value::Object(mapping)) => {
            let Some(value) = mapping.get(INPUT) else {
                return;
            };
            let input_pointer = crate::pointer(&with_pointer, INPUT);
            match classify_input(value) {
                InputState::Disabled => {}
                InputState::Enabled => out.push(enabled_finding(&input_pointer)),
                InputState::Unknown => out.push(unknown_finding(
                    &input_pointer,
                    "the `allow-unsafe-pr-checkout` value is an expression",
                )),
            }
        }
        Some(Value::String(raw)) if is_fenced_expression(raw) => out.push(unknown_finding(
            &with_pointer,
            "the entire `with:` mapping is an expression, so the checkout input cannot be checked",
        )),
        // A non-mapping, non-expression `with:` is invalid Actions syntax;
        // schema validation owns that, not this audit.
        Some(_) => {}
    }
}

enum InputState {
    /// The input is present and literally `false`.
    Disabled,
    /// The input is present and truthy, or is a literal the action coerces
    /// truthy.
    Enabled,
    /// The input is an expression that could evaluate either way.
    Unknown,
}

/// Mirrors zizmor's boolean-control evaluation: only `false` disables the
/// input, and only a wholly fenced `${{ … }}` string is unknown. Everything
/// else present — including an empty value, a number, or `"yes"` — is
/// pessimistically treated as enabled.
fn classify_input(value: &Value) -> InputState {
    match value {
        Value::Bool(false) => InputState::Disabled,
        Value::String(text) if is_fenced_expression(text) => InputState::Unknown,
        Value::String(text) if text == "false" => InputState::Disabled,
        _ => InputState::Enabled,
    }
}

fn is_fenced_expression(text: &str) -> bool {
    text.strip_prefix("${{")
        .and_then(|inner| inner.strip_suffix("}}"))
        .is_some()
}

fn enabled_finding(pointer: &str) -> Diagnostic {
    let mut finding = crate::finding(
        RULE,
        pointer,
        "`actions/checkout` is configured with `allow-unsafe-pr-checkout`, disabling its \
         fork-checkout protection",
        "Remove `allow-unsafe-pr-checkout` (or set it to `false`). actions/checkout v7+ blocks \
         checking out fork-originated code on `pull_request_target` and `workflow_run` by \
         default; this input turns that protection off.",
    );
    finding.severity = Severity::Error;
    finding.confidence = Confidence::High;
    finding
}

fn unknown_finding(pointer: &str, reason: &str) -> Diagnostic {
    let mut finding = crate::finding(
        RULE,
        pointer,
        format!("`actions/checkout` may disable its fork-checkout protection: {reason}"),
        "Pin the `allow-unsafe-pr-checkout` input to a literal `false` so this check can verify \
         that actions/checkout's fork-code protection stays enabled.",
    );
    finding.severity = Severity::Warning;
    finding.confidence = Confidence::Medium;
    finding
}

/// Only a literal `false` condition disables a job/step, matching the rest of
/// this crate's audits (and `preloop_gha_lint`'s documented policy).
fn is_disabled(value: &Value) -> bool {
    match value.get("if") {
        Some(Value::Bool(false)) => true,
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            let inner = trimmed
                .strip_prefix("${{")
                .and_then(|s| s.strip_suffix("}}"))
                .map(str::trim)
                .unwrap_or(trimmed);
            inner.eq_ignore_ascii_case("false")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(doc: &Value) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        check(doc, &AuditContext::default(), &mut out);
        out
    }

    fn checkout_step(with: Value) -> Value {
        json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        {
                            "uses": "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
                            "with": with
                        }
                    ]
                }
            }
        })
    }

    #[test]
    fn literal_true_is_an_error() {
        let findings = run(&checkout_step(json!({ "allow-unsafe-pr-checkout": true })));
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.rule_id, RULE);
        assert_eq!(finding.severity, Severity::Error);
        assert_eq!(finding.confidence, Confidence::High);
        assert_eq!(
            finding.path,
            "/jobs/build/steps/0/with/allow-unsafe-pr-checkout"
        );
    }

    #[test]
    fn truthy_literal_is_an_error() {
        let findings = run(&checkout_step(json!({ "allow-unsafe-pr-checkout": "yes" })));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
    }

    #[test]
    fn expression_is_a_warning() {
        let findings = run(&checkout_step(
            json!({ "allow-unsafe-pr-checkout": "${{ github.event_name == 'push' }}" }),
        ));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert_eq!(findings[0].confidence, Confidence::Medium);
    }

    #[test]
    fn opaque_with_mapping_is_a_warning() {
        let findings = run(&checkout_step(json!("${{ fromJSON('{}') }}")));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert_eq!(findings[0].path, "/jobs/build/steps/0/with");
    }

    #[test]
    fn literal_false_is_clean() {
        let findings = run(&checkout_step(json!({ "allow-unsafe-pr-checkout": false })));
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn absent_input_is_clean() {
        let findings = run(&checkout_step(json!({ "persist-credentials": false })));
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn other_action_is_ignored() {
        let doc = json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        {
                            "uses": "actions/cache@v4",
                            "with": { "allow-unsafe-pr-checkout": true }
                        }
                    ]
                }
            }
        });
        assert!(run(&doc).is_empty());
    }

    #[test]
    fn checkout_subpath_is_ignored() {
        let doc = json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        {
                            "uses": "actions/checkout/some-dir@v4",
                            "with": { "allow-unsafe-pr-checkout": true }
                        }
                    ]
                }
            }
        });
        assert!(run(&doc).is_empty());
    }

    #[test]
    fn disabled_step_is_ignored() {
        let doc = json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        {
                            "if": false,
                            "uses": "actions/checkout@v4",
                            "with": { "allow-unsafe-pr-checkout": true }
                        }
                    ]
                }
            }
        });
        assert!(run(&doc).is_empty());
    }

    #[test]
    fn disabled_job_is_ignored() {
        let doc = json!({
            "jobs": {
                "build": {
                    "if": false,
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        {
                            "uses": "actions/checkout@v4",
                            "with": { "allow-unsafe-pr-checkout": true }
                        }
                    ]
                }
            }
        });
        assert!(run(&doc).is_empty());
    }

    #[test]
    fn owner_and_repo_match_case_insensitively() {
        let doc = json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        {
                            "uses": "Actions/Checkout@v4",
                            "with": { "allow-unsafe-pr-checkout": true }
                        }
                    ]
                }
            }
        });
        assert_eq!(run(&doc).len(), 1);
    }
}

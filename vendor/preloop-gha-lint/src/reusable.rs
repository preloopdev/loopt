//! Reusable-workflow call checks against the callee's `on.workflow_call`
//! declaration.
//!
//! The caller supplies the callee documents because this crate performs no
//! I/O. Local calls (`./.github/workflows/x.yml` or GitHub's self-repository
//! `$/.github/workflows/x.yml`) are resolved from the repository root by the
//! caller; remote calls are unknown unless an online resolver fetched them.
//! Input, secret and output names match case-insensitively, as GitHub does.

use std::collections::{BTreeMap, BTreeSet};

use preloop_gha_authoring::WorkflowDocument;
use serde_json::Value;

use crate::{Confidence, Diagnostic, Severity, is_disabled};

/// Documents of reusable workflows called through `jobs.<id>.uses`.
///
/// Keys are the literal `uses:` values. A local call whose value is absent
/// from [`documents`](Self::documents) is reported as a missing file; any
/// other absent call is treated as unknown and skipped. [`unavailable`]
/// records remote calls an online resolver could not fetch, so the caller can
/// report them as unavailable rather than clean.
///
/// [`unavailable`]: Self::unavailable
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReusableWorkflows {
    /// Raw YAML source of each resolved callee, keyed by its `uses:` value.
    pub documents: BTreeMap<String, String>,
    /// Remote `uses:` values that could not be fetched, keyed by value, with
    /// the failure reason.
    pub unavailable: BTreeMap<String, String>,
}

/// Check every reusable-workflow call job and return the outputs each call
/// job exposes to downstream `needs.<job>.outputs` references.
///
/// A value of `None` means the callee's outputs are unknown (unresolved,
/// dynamic, or not declared), so downstream output references cannot be
/// checked.
pub(crate) fn check(
    document: &Value,
    reusable: Option<&ReusableWorkflows>,
    findings: &mut Vec<Diagnostic>,
) -> BTreeMap<String, Option<BTreeSet<String>>> {
    let mut call_outputs = BTreeMap::new();
    let Some(reusable) = reusable else {
        return call_outputs;
    };
    let Some(jobs) = document.get("jobs").and_then(Value::as_object) else {
        return call_outputs;
    };

    let mut declarations: BTreeMap<&str, Option<Declaration>> = BTreeMap::new();
    for (job_id, job) in jobs {
        if !job.is_object() || is_disabled(job) {
            continue;
        }
        let Some(Value::String(uses)) = job.get("uses") else {
            continue;
        };
        // A dynamic call target cannot be resolved statically.
        if uses.contains("${{") {
            continue;
        }
        let path = format!("/jobs/{}/uses", escape(job_id));
        let Some(source) = reusable.documents.get(uses) else {
            if is_local(uses) {
                findings.push(error(
                    "preloop/reusable-callee-missing",
                    &path,
                    format!("reusable workflow `{uses}` does not exist in this repository"),
                    "check the path; local reusable workflows are resolved from the repository root",
                ));
            }
            continue;
        };

        let declaration = declarations
            .entry(uses.as_str())
            .or_insert_with(|| Declaration::parse(source));
        let Some(declaration) = declaration.as_ref() else {
            // A callee we cannot parse is unknown; linting the callee itself
            // reports its syntax errors.
            continue;
        };
        if !declaration.callable {
            findings.push(error(
                "preloop/reusable-not-callable",
                &path,
                format!("reusable workflow `{uses}` has no `workflow_call` trigger"),
                "add `on: workflow_call` to the called workflow",
            ));
            continue;
        }

        call_outputs.insert(job_id.to_ascii_lowercase(), declaration.outputs.clone());
        check_inputs(job, job_id, uses, declaration, findings);
        check_secrets(job, job_id, uses, declaration, findings);
    }
    call_outputs
}

fn check_inputs(
    job: &Value,
    job_id: &str,
    uses: &str,
    declaration: &Declaration,
    findings: &mut Vec<Diagnostic>,
) {
    let Some(inputs) = declaration.inputs.as_ref() else {
        return;
    };
    let job_path = format!("/jobs/{}", escape(job_id));
    let supplied = match job.get("with") {
        None | Some(Value::Null) => None,
        Some(Value::Object(map)) => {
            // A dynamic key makes the supplied set unknowable.
            if map.keys().any(|name| name.contains("${{")) {
                return;
            }
            Some(map)
        }
        // `with:` itself is dynamic or malformed; required inputs cannot be
        // proven missing.
        Some(_) => return,
    };

    let mut passed = BTreeSet::new();
    if let Some(map) = supplied {
        for (name, value) in map {
            let key = name.to_ascii_lowercase();
            passed.insert(key.clone());
            let Some(input) = inputs.get(&key) else {
                findings.push(error(
                    "preloop/reusable-unknown-input",
                    &format!("{job_path}/with/{}", escape(name)),
                    format!("input `{name}` is not declared by reusable workflow `{uses}`"),
                    "pass only inputs declared under `on.workflow_call.inputs` in the called workflow",
                ));
                continue;
            };
            let Some(declared) = input.kind else {
                continue;
            };
            let Some(actual) = literal_kind(value) else {
                // Expressions are resolved at run time.
                continue;
            };
            if !accepts(declared, &actual) {
                findings.push(error(
                    "preloop/reusable-input-type",
                    &format!("{job_path}/with/{}", escape(name)),
                    format!(
                        "input `{name}` is declared as `{}` by reusable workflow `{uses}`, but the value is a `{}`",
                        declared.name(),
                        actual.name()
                    ),
                    "pass a literal of the declared type, or use an expression the callee can convert",
                ));
            }
        }
    }

    for (name, input) in inputs {
        if input.required && !input.has_default && !passed.contains(name) {
            findings.push(error(
                "preloop/reusable-missing-input",
                &format!("{job_path}/uses"),
                format!("required input `{name}` is not passed to reusable workflow `{uses}`"),
                "pass the input in `with:`, or give it a `default` in the called workflow",
            ));
        }
    }
}

fn check_secrets(
    job: &Value,
    job_id: &str,
    uses: &str,
    declaration: &Declaration,
    findings: &mut Vec<Diagnostic>,
) {
    let Some(secrets) = declaration.secrets.as_ref() else {
        return;
    };
    let job_path = format!("/jobs/{}", escape(job_id));
    let supplied = match job.get("secrets") {
        // `inherit` passes every secret; required and unknown names cannot be
        // checked.
        Some(Value::String(text)) if text.eq_ignore_ascii_case("inherit") => return,
        None | Some(Value::Null) => None,
        Some(Value::Object(map)) => {
            if map.keys().any(|name| name.contains("${{")) {
                return;
            }
            Some(map)
        }
        Some(_) => return,
    };

    let mut passed = BTreeSet::new();
    if let Some(map) = supplied {
        for name in map.keys() {
            let key = name.to_ascii_lowercase();
            passed.insert(key.clone());
            if !secrets.contains_key(&key) {
                findings.push(error(
                    "preloop/reusable-unknown-secret",
                    &format!("{job_path}/secrets/{}", escape(name)),
                    format!("secret `{name}` is not declared by reusable workflow `{uses}`"),
                    "pass only secrets declared under `on.workflow_call.secrets` in the called workflow, or use `secrets: inherit`",
                ));
            }
        }
    }

    for (name, required) in secrets {
        if *required && !passed.contains(name) {
            findings.push(error(
                "preloop/reusable-missing-secret",
                &format!("{job_path}/uses"),
                format!("required secret `{name}` is not passed to reusable workflow `{uses}`"),
                "pass the secret in `secrets:`, or use `secrets: inherit`",
            ));
        }
    }
}

fn is_local(uses: &str) -> bool {
    (uses.starts_with("./") || uses.starts_with("$/")) && !uses.contains('@')
}

/// The `on.workflow_call` declaration of a reusable workflow.
struct Declaration {
    callable: bool,
    /// Declared inputs; `None` means the declaration is dynamic.
    inputs: Option<BTreeMap<String, InputDeclaration>>,
    /// Declared secrets mapped to whether they are required; `None` means the
    /// declaration is dynamic.
    secrets: Option<BTreeMap<String, bool>>,
    /// Declared output names; `None` means absent or dynamic.
    outputs: Option<BTreeSet<String>>,
}

struct InputDeclaration {
    required: bool,
    has_default: bool,
    kind: Option<InputKind>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum InputKind {
    String,
    Number,
    Boolean,
}

impl InputKind {
    fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
        }
    }
}

enum ValueKind {
    String,
    Number,
    Boolean,
    Null,
}

impl ValueKind {
    fn name(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Null => "null",
        }
    }
}

impl Declaration {
    fn parse(source: &str) -> Option<Self> {
        let document = WorkflowDocument::parse(source).ok()?;
        let value = document.value();
        let Some(on) = value.get("on") else {
            return Some(Self {
                callable: false,
                inputs: Some(BTreeMap::new()),
                secrets: Some(BTreeMap::new()),
                outputs: None,
            });
        };
        let callable = match on {
            Value::String(event) if event.contains("${{") => return None,
            Value::String(event) => event.eq_ignore_ascii_case("workflow_call"),
            Value::Array(events) => {
                let mut callable = false;
                for event in events {
                    let event = event.as_str()?;
                    if event.contains("${{") {
                        return None;
                    }
                    callable |= event.eq_ignore_ascii_case("workflow_call");
                }
                callable
            }
            Value::Object(events) => {
                let mut callable = false;
                for (event, config) in events {
                    if event.contains("${{") {
                        return None;
                    }
                    if event.eq_ignore_ascii_case("workflow_call") {
                        callable = true;
                        if config.as_str().is_some_and(|text| text.contains("${{")) {
                            return None;
                        }
                    }
                }
                callable
            }
            _ => return None,
        };

        let config = match on {
            Value::Object(events) => events
                .iter()
                .find(|(event, _)| event.eq_ignore_ascii_case("workflow_call"))
                .map(|(_, config)| config),
            _ => None,
        };
        let (inputs, secrets, outputs) = match config {
            None | Some(Value::Null) => (Some(BTreeMap::new()), Some(BTreeMap::new()), None),
            Some(Value::Object(config)) => (
                inputs(config.get("inputs")),
                secrets(config.get("secrets")),
                outputs(config.get("outputs")),
            ),
            Some(_) => (None, None, None),
        };
        Some(Self {
            callable,
            inputs,
            secrets,
            outputs,
        })
    }
}

fn inputs(value: Option<&Value>) -> Option<BTreeMap<String, InputDeclaration>> {
    let Some(value) = value else {
        return Some(BTreeMap::new());
    };
    let Some(map) = value.as_object() else {
        return value.is_null().then(BTreeMap::new);
    };
    let mut declarations = BTreeMap::new();
    for (name, input) in map {
        if name.contains("${{") {
            return None;
        }
        let Some(input) = input.as_object() else {
            declarations.insert(
                name.to_ascii_lowercase(),
                InputDeclaration {
                    required: false,
                    has_default: false,
                    kind: None,
                },
            );
            continue;
        };
        let required = match input.get("required") {
            Some(Value::Bool(required)) => *required,
            _ => false,
        };
        let has_default = input.get("default").is_some_and(|value| !value.is_null());
        let kind = match input.get("type").and_then(Value::as_str) {
            Some("string") => Some(InputKind::String),
            Some("number") => Some(InputKind::Number),
            Some("boolean") => Some(InputKind::Boolean),
            _ => None,
        };
        declarations.insert(
            name.to_ascii_lowercase(),
            InputDeclaration {
                required,
                has_default,
                kind,
            },
        );
    }
    Some(declarations)
}

fn secrets(value: Option<&Value>) -> Option<BTreeMap<String, bool>> {
    let Some(value) = value else {
        return Some(BTreeMap::new());
    };
    let Some(map) = value.as_object() else {
        return value.is_null().then(BTreeMap::new);
    };
    let mut declarations = BTreeMap::new();
    for (name, secret) in map {
        if name.contains("${{") {
            return None;
        }
        let required = secret
            .as_object()
            .and_then(|secret| secret.get("required"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        declarations.insert(name.to_ascii_lowercase(), required);
    }
    Some(declarations)
}

fn outputs(value: Option<&Value>) -> Option<BTreeSet<String>> {
    let value = value?;
    if value.is_null() {
        return Some(BTreeSet::new());
    }
    let map = value.as_object()?;
    let mut names = BTreeSet::new();
    for name in map.keys() {
        if name.contains("${{") {
            return None;
        }
        names.insert(name.to_ascii_lowercase());
    }
    Some(names)
}

fn literal_kind(value: &Value) -> Option<ValueKind> {
    match value {
        Value::String(text) if text.contains("${{") => None,
        Value::String(_) => Some(ValueKind::String),
        Value::Number(_) => Some(ValueKind::Number),
        Value::Bool(_) => Some(ValueKind::Boolean),
        Value::Null => Some(ValueKind::Null),
        _ => None,
    }
}

/// GitHub accepts numbers where strings are declared (they are converted),
/// but does not convert booleans to strings or strings to numbers/booleans.
fn accepts(declared: InputKind, actual: &ValueKind) -> bool {
    matches!(
        (declared, actual),
        (InputKind::String, ValueKind::String | ValueKind::Number)
            | (InputKind::Number, ValueKind::Number)
            | (InputKind::Boolean, ValueKind::Boolean)
    )
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{Diagnostic, LintContext, lint_workflow_with_reusable};

    use super::ReusableWorkflows;

    const USES: &str = "./.github/workflows/callee.yml";

    fn lint(caller: Value, callee: &str) -> Vec<Diagnostic> {
        let mut reusable = ReusableWorkflows::default();
        reusable
            .documents
            .insert(USES.to_owned(), callee.to_owned());
        lint_with(caller, reusable)
    }

    fn lint_with(caller: Value, reusable: ReusableWorkflows) -> Vec<Diagnostic> {
        let document =
            preloop_gha_authoring::WorkflowDocument::from_value(caller).expect("workflow document");
        lint_workflow_with_reusable(&document, &LintContext::default(), Some(&reusable))
    }

    fn caller(with: Value, secrets: Value) -> Value {
        json!({
            "name": "ci",
            "on": "push",
            "jobs": {
                "call": {
                    "uses": USES,
                    "with": with,
                    "secrets": secrets,
                }
            }
        })
    }

    fn rules<'a>(findings: &'a [Diagnostic], rule: &str) -> Vec<&'a Diagnostic> {
        findings
            .iter()
            .filter(|finding| finding.rule_id == rule)
            .collect()
    }

    const CALLEE: &str = r#"
on:
  workflow_call:
    inputs:
      Name:
        type: string
        required: true
      count:
        type: number
    secrets:
      token:
        required: true
"#;

    #[test]
    fn unknown_and_missing_inputs_and_secrets_are_reported() {
        let findings = lint(caller(json!({"bogus": "x"}), json!({})), CALLEE);
        let unknown = rules(&findings, "preloop/reusable-unknown-input");
        assert_eq!(unknown.len(), 1, "{unknown:#?}");
        assert_eq!(unknown[0].path, "/jobs/call/with/bogus");
        let missing = rules(&findings, "preloop/reusable-missing-input");
        assert_eq!(missing.len(), 1, "{missing:#?}");
        assert_eq!(missing[0].path, "/jobs/call/uses");
        let missing_secret = rules(&findings, "preloop/reusable-missing-secret");
        assert_eq!(missing_secret.len(), 1, "{missing_secret:#?}");
        assert_eq!(missing_secret[0].path, "/jobs/call/uses");
    }

    #[test]
    fn declared_inputs_and_secrets_pass_case_insensitively() {
        let findings = lint(
            caller(json!({"name": "hi", "COUNT": 3}), json!({"TOKEN": "x"})),
            CALLEE,
        );
        assert!(
            findings
                .iter()
                .all(|finding| !finding.rule_id.starts_with("preloop/reusable-")),
            "{findings:#?}"
        );
    }

    #[test]
    fn a_default_makes_a_required_input_optional_at_the_call_site() {
        let callee = r#"
on:
  workflow_call:
    inputs:
      flag:
        type: boolean
        required: true
        default: false
"#;
        let findings = lint(caller(json!({}), json!({})), callee);
        assert!(
            rules(&findings, "preloop/reusable-missing-input").is_empty(),
            "{findings:#?}"
        );
    }

    #[test]
    fn literal_input_types_are_checked_and_expressions_are_unknown() {
        let callee = r#"
on:
  workflow_call:
    inputs:
      count:
        type: number
      message:
        type: string
      flag:
        type: boolean
"#;
        let findings = lint(
            caller(
                json!({
                    "count": true,
                    "message": null,
                    "flag": "yes"
                }),
                json!({}),
            ),
            callee,
        );
        let types = rules(&findings, "preloop/reusable-input-type");
        let paths: Vec<&str> = types.iter().map(|finding| finding.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/jobs/call/with/count",
                "/jobs/call/with/flag",
                "/jobs/call/with/message"
            ],
            "{types:#?}"
        );

        let findings = lint(
            caller(
                json!({
                    "count": "${{ inputs.count }}",
                    "message": 3,
                    "flag": "${{ inputs.flag }}"
                }),
                json!({}),
            ),
            callee,
        );
        assert!(
            rules(&findings, "preloop/reusable-input-type").is_empty(),
            "{findings:#?}"
        );
    }

    #[test]
    fn inheriting_secrets_skips_secret_checks() {
        let findings = lint(caller(json!({"name": "hi"}), json!("inherit")), CALLEE);
        assert!(
            rules(&findings, "preloop/reusable-missing-secret").is_empty(),
            "{findings:#?}"
        );
        assert!(
            rules(&findings, "preloop/reusable-unknown-secret").is_empty(),
            "{findings:#?}"
        );
    }

    #[test]
    fn unknown_secret_is_reported_when_not_inheriting() {
        let findings = lint(caller(json!({"name": "hi"}), json!({"bogus": "x"})), CALLEE);
        let unknown = rules(&findings, "preloop/reusable-unknown-secret");
        assert_eq!(unknown.len(), 1, "{unknown:#?}");
        assert_eq!(unknown[0].path, "/jobs/call/secrets/bogus");
    }

    #[test]
    fn a_local_callee_that_cannot_be_called_is_reported() {
        let callee = "on: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n";
        let findings = lint(caller(json!({}), json!({})), callee);
        let not_callable = rules(&findings, "preloop/reusable-not-callable");
        assert_eq!(not_callable.len(), 1, "{findings:#?}");
        assert_eq!(not_callable[0].path, "/jobs/call/uses");
    }

    #[test]
    fn a_missing_local_callee_is_reported() {
        let findings = lint_with(caller(json!({}), json!({})), ReusableWorkflows::default());
        let missing = rules(&findings, "preloop/reusable-callee-missing");
        assert_eq!(missing.len(), 1, "{findings:#?}");
        assert_eq!(missing[0].path, "/jobs/call/uses");
    }

    #[test]
    fn self_repository_calls_use_the_same_checks() {
        let uses = "$/.github/workflows/callee.yml";
        let mut reusable = ReusableWorkflows::default();
        reusable
            .documents
            .insert(uses.to_owned(), CALLEE.to_owned());
        let findings = lint_with(
            json!({
                "name": "ci",
                "on": "push",
                "jobs": {
                    "call": {
                        "uses": uses,
                        "with": {"bogus": "x"}
                    }
                }
            }),
            reusable,
        );
        let unknown = rules(&findings, "preloop/reusable-unknown-input");
        assert_eq!(unknown.len(), 1, "{findings:#?}");
        let missing = rules(&findings, "preloop/reusable-missing-input");
        assert_eq!(missing.len(), 1, "{findings:#?}");
    }

    #[test]
    fn dynamic_call_targets_and_inputs_are_skipped() {
        let callee = r#"
on:
  workflow_call:
    inputs:
      name:
        type: string
        required: true
"#;
        let findings = lint(
            json!({
                "name": "ci",
                "on": "push",
                "jobs": {
                    "call": {
                        "uses": "${{ matrix.workflow }}",
                        "with": {"bogus": "x"}
                    }
                }
            }),
            callee,
        );
        assert!(
            findings
                .iter()
                .all(|finding| !finding.rule_id.starts_with("preloop/reusable-")),
            "{findings:#?}"
        );

        let findings = lint(
            json!({
                "name": "ci",
                "on": "push",
                "jobs": {
                    "call": {
                        "uses": USES,
                        "with": "${{ fromJSON(inputs.arguments) }}"
                    }
                }
            }),
            callee,
        );
        assert!(
            rules(&findings, "preloop/reusable-missing-input").is_empty(),
            "{findings:#?}"
        );
    }

    #[test]
    fn unknown_outputs_of_call_jobs_are_reported() {
        let callee = r#"
on:
  workflow_call:
    outputs:
      tag:
        value: ${{ jobs.build.outputs.tag }}
"#;
        let caller = json!({
            "name": "ci",
            "on": "push",
            "jobs": {
                "call": {"uses": USES},
                "deploy": {
                    "needs": "call",
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        {"run": "echo ${{ needs.call.outputs.tag }}"},
                        {"run": "echo ${{ needs.call.outputs.bogus }}"}
                    ]
                }
            }
        });
        let findings = lint(caller, callee);
        let unknown = rules(&findings, "preloop/reusable-unknown-output");
        assert_eq!(unknown.len(), 1, "{unknown:#?}");
        assert_eq!(unknown[0].path, "/jobs/deploy/steps/1/run");
    }

    #[test]
    fn a_callee_without_declared_outputs_leaves_output_references_unknown() {
        let callee = "on:\n  workflow_call:\n";
        let caller = json!({
            "name": "ci",
            "on": "push",
            "jobs": {
                "call": {"uses": USES},
                "deploy": {
                    "needs": "call",
                    "runs-on": "ubuntu-latest",
                    "steps": [{"run": "echo ${{ needs.call.outputs.tag }}"}]
                }
            }
        });
        let findings = lint(caller, callee);
        assert!(
            rules(&findings, "preloop/reusable-unknown-output").is_empty(),
            "{findings:#?}"
        );
    }
}

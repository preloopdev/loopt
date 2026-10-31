use std::collections::BTreeSet;

use preloop_gha_expressions::collect_secret_reads;
use preloop_gha_parser::{ENGINE_PROVIDED_SECRETS, WorkflowDocument};
use serde_json::Value;

use crate::{Diagnostic, LintContext, diagnostic, expressions, is_disabled};

pub(crate) fn check(
    document: &WorkflowDocument,
    context: &LintContext,
    findings: &mut Vec<Diagnostic>,
) {
    let workflow = document.value();
    let Some(jobs) = workflow.get("jobs").and_then(Value::as_object) else {
        return;
    };
    // workflow_call declarations name caller-supplied inputs, not stored secrets.
    let supplied = workflow
        .pointer("/on/workflow_call/secrets")
        .and_then(Value::as_object);
    for (id, job) in jobs {
        if !job.is_object() || is_disabled(job) {
            continue;
        }
        let path = child("/jobs", id);
        let reusable = job.get("uses").is_some();
        if !reusable {
            check_runner(job, &path, context, findings);
            check_environment(job, &path, context, findings);
        }
        let Some(base) = context.available_secrets.as_ref() else {
            continue;
        };
        // A reusable call evaluates the caller's secret map, not the callee's
        // steps/environment (nor the caller's otherwise-inherited workflow env).
        let environment = if reusable {
            None
        } else {
            match environment_name(job) {
                Environment::None => None,
                Environment::Literal(name, _) => {
                    let Some(names) = context.environment_secrets.get(name) else {
                        // No inventory was supplied for this environment tier.
                        continue;
                    };
                    Some(names)
                }
                Environment::Unknown => continue,
            }
        };
        let scope = SecretScope {
            base,
            environment,
            supplied,
        };
        if reusable {
            for key in ["secrets", "with", "name", "concurrency", "if"] {
                scan_field(job, key, &path, &scope, findings);
            }
            continue;
        }
        if let Some(env) = workflow.get("env").and_then(Value::as_object) {
            let overrides = job.get("env").and_then(Value::as_object);
            for (name, value) in env {
                if !overrides.is_some_and(|env| env.contains_key(name)) {
                    scan(value, &child("/env", name), false, &scope, findings);
                }
            }
        }
        for key in [
            "env",
            "name",
            "if",
            "container",
            "services",
            "environment",
            "outputs",
            "concurrency",
        ] {
            scan_field(job, key, &path, &scope, findings);
        }
        if let Some(steps) = job.get("steps").and_then(Value::as_array) {
            for (index, step) in steps.iter().enumerate() {
                if is_disabled(step) {
                    continue;
                }
                let step_path = format!("{path}/steps/{index}");
                for key in [
                    "name",
                    "run",
                    "env",
                    "with",
                    "if",
                    "working-directory",
                    "shell",
                ] {
                    scan_field(step, key, &step_path, &scope, findings);
                }
            }
        }
    }
}

fn check_runner(job: &Value, path: &str, context: &LintContext, findings: &mut Vec<Diagnostic>) {
    let Some(runners) = context.runner_label_sets.as_ref() else {
        return;
    };
    let Some(selector) = job.get("runs-on") else {
        return;
    };
    let labels = if let Some(object) = selector.as_object() {
        if object
            .get("group")
            .and_then(Value::as_str)
            .is_some_and(|group| group.contains("${{"))
        {
            return;
        }
        let Some(labels) = object.get("labels") else {
            return;
        };
        labels
    } else {
        selector
    };
    if !literal_labels(labels) {
        return;
    }
    if runners.iter().any(|runner| matches_runner(labels, runner)) {
        return;
    }
    findings.push(diagnostic(
        "unsatisfied-runner",
        &format!("{path}/runs-on"),
        "No runner in the supplied inventory satisfies these literal runs-on labels.".to_owned(),
        "Register a matching runner or update runs-on; the inventory must include provisionable runners as well as currently registered runners.",
    ));
}

fn literal_labels(value: &Value) -> bool {
    match value {
        Value::String(label) => !label.trim().is_empty() && !label.contains("${{"),
        Value::Array(labels) => {
            !labels.is_empty()
                && labels.iter().all(|label| {
                    label
                        .as_str()
                        .is_some_and(|label| !label.trim().is_empty() && !label.contains("${{"))
                })
        }
        _ => false,
    }
}

// Keep the scheduler's hosted-image stand-in behavior, including unknown
// (empty) runner labels and the Linux/macOS/Windows precedence for OS labels.
fn matches_runner(labels: &Value, runner: &BTreeSet<String>) -> bool {
    if runner.is_empty() {
        return true;
    }
    let os = ["linux", "macos", "windows"]
        .into_iter()
        .find(|os| contains_name(runner, os));
    let matches = |required: &str| {
        if contains_name(runner, required) {
            return true;
        }
        let required_os = [
            ("ubuntu", "linux"),
            ("macos", "macos"),
            ("windows", "windows"),
        ]
        .into_iter()
        .find_map(|(prefix, os)| {
            required
                .get(..prefix.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
                .then_some(os)
        });
        match (required_os, os) {
            (Some(required), Some(actual)) => required == actual,
            (Some(_), None) => contains_name(runner, "self-hosted"),
            (None, _) => false,
        }
    };
    match labels {
        Value::String(label) => matches(label),
        Value::Array(labels) => labels
            .iter()
            .all(|label| label.as_str().is_some_and(matches)),
        _ => false,
    }
}

enum Environment<'a> {
    None,
    Literal(&'a str, bool),
    Unknown,
}

fn environment_name(job: &Value) -> Environment<'_> {
    let Some(environment) = job.get("environment") else {
        return Environment::None;
    };
    if environment.is_null() {
        return Environment::None;
    }
    let (name, object) = match environment {
        Value::String(name) => (Some(name.as_str()), false),
        Value::Object(map) => (map.get("name").and_then(Value::as_str), true),
        _ => return Environment::Unknown,
    };
    match name.map(str::trim) {
        Some("") => Environment::None,
        Some(name) if !name.contains("${{") => Environment::Literal(name, object),
        _ => Environment::Unknown,
    }
}

fn check_environment(
    job: &Value,
    path: &str,
    context: &LintContext,
    findings: &mut Vec<Diagnostic>,
) {
    let Some(known) = context.known_environments.as_ref() else {
        return;
    };
    let Environment::Literal(name, object) = environment_name(job) else {
        return;
    };
    // Preloop's built-in secret provider and local environment rule map use
    // exact environment keys; unlike secret names these are case-sensitive.
    if known.contains(name) {
        return;
    }
    let path = if object {
        format!("{path}/environment/name")
    } else {
        format!("{path}/environment")
    };
    findings.push(diagnostic(
        "unknown-environment",
        &path,
        format!("Environment {name:?} is absent from the supplied environment registry."),
        "Check the environment name and configure its deployment rules and secret inventory; an unknown environment may otherwise run without the intended protections.",
    ));
}

struct SecretScope<'a> {
    base: &'a BTreeSet<String>,
    environment: Option<&'a BTreeSet<String>>,
    supplied: Option<&'a serde_json::Map<String, Value>>,
}

impl SecretScope<'_> {
    fn contains(&self, name: &str) -> bool {
        contains_name(self.base, name)
            || self
                .environment
                .is_some_and(|names| contains_name(names, name))
            || self
                .supplied
                .is_some_and(|names| names.keys().any(|key| key.eq_ignore_ascii_case(name)))
            || ENGINE_PROVIDED_SECRETS
                .iter()
                .any(|provided| provided.eq_ignore_ascii_case(name))
    }
}

fn contains_name(names: &BTreeSet<String>, name: &str) -> bool {
    names.iter().any(|known| {
        known
            .chars()
            .flat_map(char::to_lowercase)
            .eq(name.chars().flat_map(char::to_lowercase))
    })
}

fn scan_field(
    object: &Value,
    key: &str,
    path: &str,
    scope: &SecretScope<'_>,
    findings: &mut Vec<Diagnostic>,
) {
    if let Some(value) = object.get(key) {
        scan(value, &child(path, key), key == "if", scope, findings);
    }
}

fn scan(
    value: &Value,
    path: &str,
    condition: bool,
    scope: &SecretScope<'_>,
    findings: &mut Vec<Diagnostic>,
) {
    match value {
        Value::String(text) => {
            let mut missing = BTreeSet::new();
            let mut collect = |expression: &str| {
                if let Ok(reads) = collect_secret_reads(expression) {
                    // Dynamic reads do not prove a name missing. Separately
                    // collected literal fallback names are still knowable.
                    for name in reads.names {
                        if !scope.contains(&name) {
                            missing.insert(name.to_ascii_uppercase());
                        }
                    }
                }
            };
            if condition && !text.contains("${{") {
                collect(text);
            } else {
                for expression in expressions(text) {
                    collect(expression);
                }
            }
            for name in missing {
                findings.push(diagnostic(
                    "missing-secret",
                    path,
                    format!("Secret {name:?} is absent from the supplied secret inventories for this scope."),
                    "Supply this secret in the base or job environment scope, or correct the reference. Secret values are never needed for linting.",
                ));
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                scan(value, &format!("{path}/{index}"), false, scope, findings);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                scan(value, &child(path, key), false, scope, findings);
            }
        }
        _ => {}
    }
}

fn child(path: &str, key: &str) -> String {
    format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}

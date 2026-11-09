//! Workflow schema checks that do not need execution context.
//!
//! The key, scope and identifier tables mirror GitHub's workflow syntax
//! reference:
//! <https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax>
//!
//! Unknown keys are advisories, not a gate: the authoring model deliberately
//! preserves fields it does not understand. Any value or key containing an
//! expression is treated as unknown rather than wrong.

use serde_json::{Map, Value};

use crate::{Confidence, Diagnostic, Severity};

const WORKFLOW_KEYS: &[&str] = &[
    "cache-mode",
    "concurrency",
    "defaults",
    "env",
    "jobs",
    "name",
    "on",
    "permissions",
    "run-name",
];

const JOB_KEYS: &[&str] = &[
    "cache-mode",
    "concurrency",
    "container",
    "continue-on-error",
    "defaults",
    "env",
    "environment",
    "if",
    "name",
    "needs",
    "outputs",
    "permissions",
    "runs-on",
    "services",
    "snapshot",
    "steps",
    "strategy",
    "timeout-minutes",
];

const REUSABLE_JOB_KEYS: &[&str] = &[
    "cache-mode",
    "concurrency",
    "if",
    "name",
    "needs",
    "permissions",
    "secrets",
    "strategy",
    "uses",
    "with",
];

/// Reusable-call keys with dedicated conflict rules; do not report them twice.
const REUSABLE_CONFLICT_KEYS: &[&str] = &["container", "runs-on", "services", "steps"];

const STEP_KEYS: &[&str] = &[
    "background",
    "cancel",
    "continue-on-error",
    "env",
    "id",
    "if",
    "name",
    "parallel",
    "run",
    "shell",
    "timeout-minutes",
    "uses",
    "wait",
    "wait-all",
    "with",
    "working-directory",
];

const STRATEGY_KEYS: &[&str] = &["fail-fast", "matrix", "max-parallel"];

const CONTAINER_KEYS: &[&str] = &["credentials", "env", "image", "options", "ports", "volumes"];

const SERVICE_KEYS: &[&str] = &[
    "command",
    "credentials",
    "entrypoint",
    "env",
    "image",
    "options",
    "ports",
    "volumes",
];

const CREDENTIALS_KEYS: &[&str] = &["password", "username"];

const DEFAULTS_KEYS: &[&str] = &["run"];

const DEFAULTS_RUN_KEYS: &[&str] = &["shell", "working-directory"];

const CONCURRENCY_KEYS: &[&str] = &["cancel-in-progress", "group", "queue"];

const ENVIRONMENT_KEYS: &[&str] = &["deployment", "name", "url"];

const SNAPSHOT_KEYS: &[&str] = &["if", "image-name", "version"];

/// Default environment variables GitHub sets on every runner. The workflow
/// cannot overwrite them; the assignment is ignored.
/// <https://docs.github.com/en/actions/reference/workflows-and-actions/variables#default-environment-variables>
const DEFAULT_ENV_VARS: &[&str] = &[
    "GITHUB_ACTION",
    "GITHUB_ACTION_PATH",
    "GITHUB_ACTION_REPOSITORY",
    "GITHUB_ACTIONS",
    "GITHUB_ACTOR",
    "GITHUB_ACTOR_ID",
    "GITHUB_API_URL",
    "GITHUB_ARTIFACTS",
    "GITHUB_ARTIFACTS_LIST",
    "GITHUB_BASE_REF",
    "GITHUB_ENV",
    "GITHUB_EVENT_NAME",
    "GITHUB_EVENT_PATH",
    "GITHUB_GRAPHQL_URL",
    "GITHUB_HEAD_REF",
    "GITHUB_JOB",
    "GITHUB_OUTPUT",
    "GITHUB_PATH",
    "GITHUB_REF",
    "GITHUB_REF_NAME",
    "GITHUB_REF_PROTECTED",
    "GITHUB_REF_TYPE",
    "GITHUB_REPOSITORY",
    "GITHUB_REPOSITORY_ID",
    "GITHUB_REPOSITORY_OWNER",
    "GITHUB_REPOSITORY_OWNER_ID",
    "GITHUB_RETENTION_DAYS",
    "GITHUB_RUN_ATTEMPT",
    "GITHUB_RUN_ID",
    "GITHUB_RUN_NUMBER",
    "GITHUB_SERVER_URL",
    "GITHUB_SHA",
    "GITHUB_STATE",
    "GITHUB_STEP_SUMMARY",
    "GITHUB_TRIGGERING_ACTOR",
    "GITHUB_WORKFLOW",
    "GITHUB_WORKFLOW_REF",
    "GITHUB_WORKFLOW_SHA",
    "GITHUB_WORKSPACE",
];

pub(crate) fn check(doc: &Value, findings: &mut Vec<Diagnostic>) {
    let Some(root) = doc.as_object() else {
        return;
    };
    check_keys(root, WORKFLOW_KEYS, "workflow", "", &[], findings);
    check_env(root.get("env"), "/env", findings);
    check_permissions(root.get("permissions"), "/permissions", findings);
    if let Some(defaults) = root.get("defaults") {
        check_defaults(defaults, "/defaults", findings);
    }
    if let Some(concurrency) = root.get("concurrency") {
        check_keys_of(
            concurrency,
            CONCURRENCY_KEYS,
            "concurrency",
            "/concurrency",
            findings,
        );
    }

    let Some(jobs) = root.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        check_id(job_id, &child("/jobs", job_id), "job", findings);
        let Some(job) = job.as_object() else {
            continue;
        };
        check_job(job, &child("/jobs", job_id), findings);
    }
}

fn check_job(job: &Map<String, Value>, path: &str, findings: &mut Vec<Diagnostic>) {
    if job.contains_key("uses") {
        check_keys(
            job,
            REUSABLE_JOB_KEYS,
            "reusable-workflow job",
            path,
            REUSABLE_CONFLICT_KEYS,
            findings,
        );
    } else {
        check_keys(job, JOB_KEYS, "job", path, &[], findings);
    }
    check_env(job.get("env"), &child(path, "env"), findings);
    check_permissions(
        job.get("permissions"),
        &child(path, "permissions"),
        findings,
    );
    if let Some(strategy) = job.get("strategy") {
        check_keys_of(
            strategy,
            STRATEGY_KEYS,
            "strategy",
            &child(path, "strategy"),
            findings,
        );
    }
    if let Some(defaults) = job.get("defaults") {
        check_defaults(defaults, &child(path, "defaults"), findings);
    }
    if let Some(container) = job.get("container") {
        check_container(
            container,
            &child(path, "container"),
            CONTAINER_KEYS,
            "container",
            findings,
        );
    }
    if let Some(services) = job.get("services").and_then(Value::as_object) {
        for (service, value) in services {
            check_container(
                value,
                &child(&child(path, "services"), service),
                SERVICE_KEYS,
                "service",
                findings,
            );
        }
    }
    if let Some(concurrency) = job.get("concurrency") {
        check_keys_of(
            concurrency,
            CONCURRENCY_KEYS,
            "concurrency",
            &child(path, "concurrency"),
            findings,
        );
    }
    if let Some(environment) = job.get("environment") {
        check_keys_of(
            environment,
            ENVIRONMENT_KEYS,
            "environment",
            &child(path, "environment"),
            findings,
        );
    }
    if let Some(snapshot) = job.get("snapshot") {
        check_keys_of(
            snapshot,
            SNAPSHOT_KEYS,
            "snapshot",
            &child(path, "snapshot"),
            findings,
        );
    }
    if let Some(steps) = job.get("steps").and_then(Value::as_array) {
        check_steps(steps, &child(path, "steps"), findings);
    }
}

fn check_steps(steps: &[Value], path: &str, findings: &mut Vec<Diagnostic>) {
    for (index, step) in steps.iter().enumerate() {
        let Some(step) = step.as_object() else {
            continue;
        };
        let step_path = format!("{path}/{index}");
        check_keys(step, STEP_KEYS, "step", &step_path, &[], findings);
        check_env(step.get("env"), &child(&step_path, "env"), findings);
        if let Some(id) = step.get("id") {
            check_step_id(id, &child(&step_path, "id"), findings);
        }
        if let Some(parallel) = step.get("parallel").and_then(Value::as_array) {
            check_steps(parallel, &child(&step_path, "parallel"), findings);
        }
    }
}

fn check_defaults(value: &Value, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(defaults) = value.as_object() else {
        return;
    };
    check_keys(defaults, DEFAULTS_KEYS, "defaults", path, &[], findings);
    if let Some(run) = defaults.get("run") {
        check_keys_of(
            run,
            DEFAULTS_RUN_KEYS,
            "defaults.run",
            &child(path, "run"),
            findings,
        );
    }
}

fn check_container(
    value: &Value,
    path: &str,
    keys: &[&str],
    level: &str,
    findings: &mut Vec<Diagnostic>,
) {
    let Some(container) = value.as_object() else {
        return;
    };
    check_keys(container, keys, level, path, &[], findings);
    check_env(container.get("env"), &child(path, "env"), findings);
    if let Some(credentials) = container.get("credentials") {
        check_keys_of(
            credentials,
            CREDENTIALS_KEYS,
            "credentials",
            &child(path, "credentials"),
            findings,
        );
    }
}

fn check_keys_of(
    value: &Value,
    keys: &[&str],
    level: &str,
    path: &str,
    findings: &mut Vec<Diagnostic>,
) {
    if let Some(object) = value.as_object() {
        check_keys(object, keys, level, path, &[], findings);
    }
}

fn check_keys(
    object: &Map<String, Value>,
    keys: &[&str],
    level: &str,
    path: &str,
    skip: &[&str],
    findings: &mut Vec<Diagnostic>,
) {
    for key in object.keys() {
        if key.contains("${{") || skip.contains(&key.as_str()) || keys.contains(&key.as_str()) {
            continue;
        }
        let help = match closest_key(key, keys) {
            Some(suggestion) => {
                format!("did you mean `{suggestion}`? GitHub's workflow keys are case-sensitive.")
            }
            None => {
                "GitHub does not define this field; remove it or correct the spelling.".to_owned()
            }
        };
        findings.push(warning(
            "preloop/unknown-key",
            &child(path, key),
            format!("`{key}` is not a recognized {level} key in GitHub's workflow syntax"),
            &help,
        ));
    }
}

/// Suggest a valid key when a typo is close, including case-only mistakes.
fn closest_key<'a>(key: &str, keys: &[&'a str]) -> Option<&'a str> {
    let key = key.to_ascii_lowercase();
    let mut best: Option<(usize, &'a str)> = None;
    for candidate in keys {
        let distance = edit_distance(&key, &candidate.to_ascii_lowercase());
        if distance <= 2 && best.is_none_or(|(closest, _)| distance < closest) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (row, left) in left.chars().enumerate() {
        current[0] = row + 1;
        for (column, right) in right.iter().enumerate() {
            let substitution = usize::from(left != *right);
            current[column + 1] = (previous[column] + substitution)
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

fn check_id(id: &str, path: &str, kind: &str, findings: &mut Vec<Diagnostic>) {
    if id.contains("${{") || valid_id(id) {
        return;
    }
    findings.push(error(
        "preloop/invalid-id",
        path,
        format!("`{id}` is not a valid GitHub {kind} id"),
        &format!(
            "{kind} ids must start with a letter or `_` and contain only alphanumerics, `-`, or `_`"
        ),
    ));
}

fn check_step_id(value: &Value, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(id) = value.as_str() else {
        return;
    };
    check_id(id, path, "step", findings);
}

fn valid_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn check_env(value: Option<&Value>, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(env) = value.and_then(Value::as_object) else {
        return;
    };
    for name in env.keys() {
        if name.contains("${{") {
            continue;
        }
        let name_path = child(path, name);
        if name.contains('=') || name.contains('\0') {
            findings.push(error(
                "preloop/invalid-env-name",
                &name_path,
                format!("environment variable name {name:?} contains a character GitHub cannot use"),
                "environment variable names cannot contain `=` or NUL; move the text after `=` into the value",
            ));
        } else if name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_digit())
        {
            findings.push(warning(
                "preloop/invalid-env-name",
                &name_path,
                format!("environment variable name {name:?} starts with a digit"),
                "name the variable with a leading letter or `_` so shells and runtimes can address it",
            ));
        } else if DEFAULT_ENV_VARS
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
        {
            findings.push(warning(
                "preloop/invalid-env-name",
                &name_path,
                format!("environment variable name {name:?} is a default variable GitHub sets"),
                "GitHub ignores workflow assignments to default environment variables; rename it (this does not apply to `GITHUB_TOKEN`, which is a secret)",
            ));
        }
    }
}

fn check_permissions(value: Option<&Value>, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::Null => {}
        Value::String(text) => {
            if text.contains("${{") {
                return;
            }
            if matches!(text.as_str(), "read-all" | "write-all") {
                return;
            }
            findings.push(error(
                "preloop/invalid-permission",
                path,
                format!("`{text}` is not a valid whole-workflow permission value"),
                "use `read-all`, `write-all`, or a mapping of scopes",
            ));
        }
        Value::Object(scopes) => {
            for (scope, access) in scopes {
                if scope.contains("${{") {
                    continue;
                }
                let Some(allowed) = permission_values(scope) else {
                    findings.push(error(
                        "preloop/invalid-permission",
                        &child(path, scope),
                        format!("`{scope}` is not a GitHub token permission scope"),
                        "use one of the documented `GITHUB_TOKEN` scopes",
                    ));
                    continue;
                };
                let Some(access) = access.as_str() else {
                    findings.push(error(
                        "preloop/invalid-permission",
                        &child(path, scope),
                        format!("permission scope `{scope}` must be `read`, `write`, or `none`"),
                        "use a literal access level; expressions are not allowed here",
                    ));
                    continue;
                };
                if access.contains("${{") {
                    continue;
                }
                if !allowed.contains(&access) {
                    findings.push(error(
                        "preloop/invalid-permission",
                        &child(path, scope),
                        format!("`{access}` is not a valid access level for `{scope}`"),
                        &format!("use one of {}", allowed.join(", ")),
                    ));
                }
            }
        }
        _ => findings.push(error(
            "preloop/invalid-permission",
            path,
            "`permissions` must be `read-all`, `write-all`, or a mapping of scopes",
            "write the permissions as a mapping such as `contents: read`",
        )),
    }
}

fn permission_values(scope: &str) -> Option<&'static [&'static str]> {
    const ALL: &[&str] = &["read", "write", "none"];
    const WRITE: &[&str] = &["write", "none"];
    const READ: &[&str] = &["read", "none"];
    Some(match scope {
        "id-token" | "copilot-requests" => WRITE,
        "models" | "vulnerability-alerts" => READ,
        "actions"
        | "artifact-metadata"
        | "attestations"
        | "checks"
        | "code-quality"
        | "contents"
        | "deployments"
        | "discussions"
        | "issues"
        | "packages"
        | "pages"
        | "pull-requests"
        | "repository-projects"
        | "security-events"
        | "statuses" => ALL,
        _ => return None,
    })
}

fn error(rule_id: &str, path: &str, message: impl Into<String>, help: &str) -> Diagnostic {
    let mut finding = crate::diagnostic(rule_id, path, message.into(), help);
    finding.severity = Severity::Error;
    finding.confidence = Confidence::High;
    finding
}

fn warning(rule_id: &str, path: &str, message: impl Into<String>, help: &str) -> Diagnostic {
    crate::diagnostic(rule_id, path, message.into(), help)
}

fn child(path: &str, key: &str) -> String {
    format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"))
}

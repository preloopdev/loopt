//! Context-reference checks for `${{ }}` expressions and bare `if:` conditions.
//!
//! These rules catch typos GitHub silently evaluates to an empty string:
//! unknown step ids, `needs:` entries, matrix keys and declared inputs,
//! contexts used where GitHub does not provide them, and unknown properties of
//! the contexts whose property set is closed. The shared expression engine
//! supplies the AST, so computed access (`steps[format(...)]`) is unknown
//! rather than wrong.
//!
//! Literal disabled jobs/steps keep syntax checks but omit these execution
//! checks, matching the other semantic pass.

use std::collections::{BTreeMap, BTreeSet};

use preloop_gha_expressions::collect_context_paths;
use serde_json::Value;

use crate::{Confidence, Diagnostic, Severity, diagnostic, expressions, is_disabled};

/// Contexts GitHub makes available to workflow expressions. `jobs` is only
/// available in reusable-workflow outputs; the availability table enforces that.
const KNOWN_CONTEXTS: &[&str] = &[
    "env", "github", "inputs", "job", "jobs", "matrix", "needs", "runner", "secrets", "steps",
    "strategy", "vars",
];

// Closed property sets, mirroring the GitHub contexts reference
// (https://docs.github.com/en/actions/reference/workflows-and-actions/contexts)
// and cross-checked against actionlint's `expr_sema.go`.
const GITHUB_PROPERTIES: &[&str] = &[
    "action",
    "action_path",
    "action_ref",
    "action_repository",
    "action_status",
    "actor",
    "actor_id",
    "api_url",
    "artifact_cache_size_limit",
    "artifacts",
    "artifacts_list",
    "base_ref",
    "env",
    "event",
    "event_name",
    "event_path",
    "graphql_url",
    "head_ref",
    "job",
    "output",
    "path",
    "ref",
    "ref_name",
    "ref_protected",
    "ref_type",
    "repository",
    "repository_id",
    "repository_owner",
    "repository_owner_id",
    "repository_visibility",
    "repositoryurl",
    "retention_days",
    "run_attempt",
    "run_id",
    "run_number",
    "secret_source",
    "server_url",
    "sha",
    "state",
    "step_summary",
    "token",
    "triggering_actor",
    "workflow",
    "workflow_ref",
    "workflow_sha",
    "workspace",
];

const RUNNER_PROPERTIES: &[&str] = &[
    "arch",
    "debug",
    "environment",
    "name",
    "os",
    "temp",
    "tool_cache",
];

const JOB_PROPERTIES: &[&str] = &[
    "check_run_id",
    "container",
    "services",
    "status",
    "workflow_file_path",
    "workflow_ref",
    "workflow_repository",
    "workflow_sha",
];
const JOB_CONTAINER_PROPERTIES: &[&str] = &["id", "network"];
const JOB_SERVICE_PROPERTIES: &[&str] = &["id", "network", "ports"];
const STRATEGY_PROPERTIES: &[&str] = &["fail-fast", "job-index", "job-total", "max-parallel"];

// Context availability, mirroring GitHub's "Context availability" table
// (https://docs.github.com/en/actions/reference/workflows-and-actions/contexts#context-availability)
// and cross-checked against actionlint's generated `availability.go`.
const EVERYTHING: &[&str] = &[
    "env", "github", "inputs", "job", "matrix", "needs", "runner", "secrets", "steps", "strategy",
    "vars",
];
const WITHOUT_SECRETS: &[&str] = &[
    "env", "github", "inputs", "job", "matrix", "needs", "runner", "steps", "strategy", "vars",
];
const WITHOUT_STEPS: &[&str] = &[
    "env", "github", "inputs", "job", "matrix", "needs", "runner", "secrets", "strategy", "vars",
];
const WITHOUT_JOB_RUNNER_STEPS: &[&str] = &[
    "env", "github", "inputs", "matrix", "needs", "secrets", "strategy", "vars",
];
const DEFAULTS_RUN: &[&str] = &[
    "env", "github", "inputs", "matrix", "needs", "strategy", "vars",
];
const WORKFLOW_CALL_OUTPUT: &[&str] = &["github", "inputs", "jobs", "vars"];
const JOB_ENV: &[&str] = &[
    "github", "inputs", "matrix", "needs", "secrets", "strategy", "vars",
];
const JOB_SCALAR: &[&str] = &["github", "inputs", "matrix", "needs", "strategy", "vars"];
const JOB_WITHOUT_MATRIX: &[&str] = &["github", "inputs", "needs", "vars"];
const WORKFLOW_ENV: &[&str] = &["github", "inputs", "secrets", "vars"];
const WORKFLOW_SCALAR: &[&str] = &["github", "inputs", "vars"];

pub(crate) fn check(document: &Value, findings: &mut Vec<Diagnostic>) {
    let index = Index::new(document);
    walk(document, "", None, None, false, false, &index, findings);
}

/// Facts a job contributes to reference checks.
struct JobFacts {
    id: String,
    /// Step ids (lowercase) to the first step that defines them; `None` means
    /// an id is an expression or not a string, so any id may match.
    steps: Option<BTreeMap<String, usize>>,
    /// Jobs named by `needs:`; `None` means the list is dynamic.
    needs: Option<BTreeSet<String>>,
    matrix: MatrixFacts,
    /// Declared `outputs:` names; `None` means absent or dynamic.
    outputs: Option<BTreeSet<String>>,
}

struct MatrixFacts {
    present: bool,
    /// Keys an axis or an `include` entry defines; `None` means the matrix is
    /// an expression or cannot be read literally.
    keys: Option<BTreeSet<String>>,
}

struct Index {
    jobs: BTreeMap<String, JobFacts>,
    /// Declared workflow inputs; `None` means the workflow has no
    /// `workflow_dispatch`/`workflow_call` event or its declarations are dynamic.
    inputs: Option<BTreeSet<String>>,
}

impl Index {
    fn new(document: &Value) -> Self {
        let mut jobs = BTreeMap::new();
        if let Some(object) = document.get("jobs").and_then(Value::as_object) {
            for (id, job) in object {
                if let Some(job) = job.as_object() {
                    jobs.insert(id.to_ascii_lowercase(), JobFacts::new(id, job));
                }
            }
        }
        Self {
            jobs,
            inputs: inputs(document),
        }
    }
}

impl JobFacts {
    fn new(id: &str, job: &serde_json::Map<String, Value>) -> Self {
        Self {
            id: id.to_owned(),
            steps: step_ids(job.get("steps")),
            needs: needs(job.get("needs")),
            matrix: matrix(job),
            outputs: outputs(job.get("outputs")),
        }
    }
}

fn step_ids(value: Option<&Value>) -> Option<BTreeMap<String, usize>> {
    let Some(value) = value else {
        return Some(BTreeMap::new());
    };
    let steps = value.as_array()?;
    let mut ids: BTreeMap<String, usize> = BTreeMap::new();
    for (position, step) in steps.iter().enumerate() {
        let step = step.as_object()?;
        let Some(id) = step.get("id") else {
            continue;
        };
        let id = id.as_str()?;
        if id.contains("${{") {
            return None;
        }
        ids.entry(id.to_ascii_lowercase())
            .and_modify(|first| *first = (*first).min(position))
            .or_insert(position);
    }
    Some(ids)
}

fn needs(value: Option<&Value>) -> Option<BTreeSet<String>> {
    let Some(value) = value else {
        return Some(BTreeSet::new());
    };
    match value {
        Value::String(need) if !need.contains("${{") => {
            Some(BTreeSet::from([need.to_ascii_lowercase()]))
        }
        Value::Array(items) => {
            let mut needs = BTreeSet::new();
            for item in items {
                let need = item.as_str()?;
                if need.contains("${{") {
                    return None;
                }
                needs.insert(need.to_ascii_lowercase());
            }
            Some(needs)
        }
        _ => None,
    }
}

fn matrix(job: &serde_json::Map<String, Value>) -> MatrixFacts {
    let Some(value) = job
        .get("strategy")
        .and_then(|strategy| strategy.get("matrix"))
    else {
        return MatrixFacts {
            present: false,
            keys: Some(BTreeSet::new()),
        };
    };
    let Some(matrix) = value.as_object() else {
        return MatrixFacts {
            present: true,
            keys: None,
        };
    };
    let mut keys = BTreeSet::new();
    for (key, value) in matrix {
        if key.contains("${{") {
            return MatrixFacts {
                present: true,
                keys: None,
            };
        }
        match key.as_str() {
            "include" => {
                let Some(entries) = value.as_array() else {
                    return MatrixFacts {
                        present: true,
                        keys: None,
                    };
                };
                for entry in entries {
                    let Some(entry) = entry.as_object() else {
                        return MatrixFacts {
                            present: true,
                            keys: None,
                        };
                    };
                    for key in entry.keys() {
                        if key.contains("${{") {
                            return MatrixFacts {
                                present: true,
                                keys: None,
                            };
                        }
                        keys.insert(key.to_ascii_lowercase());
                    }
                }
            }
            // `exclude` filters combinations; it does not define a key.
            "exclude" => {}
            _ => {
                keys.insert(key.to_ascii_lowercase());
            }
        }
    }
    MatrixFacts {
        present: true,
        keys: Some(keys),
    }
}

fn outputs(value: Option<&Value>) -> Option<BTreeSet<String>> {
    let value = value?;
    let outputs = value.as_object()?;
    let mut names = BTreeSet::new();
    for key in outputs.keys() {
        if key.contains("${{") {
            return None;
        }
        names.insert(key.to_ascii_lowercase());
    }
    Some(names)
}

fn inputs(document: &Value) -> Option<BTreeSet<String>> {
    let on = document.get("on")?;
    let events: BTreeSet<&str> = match on {
        Value::String(event) => BTreeSet::from([event.as_str()]),
        Value::Array(events) => events.iter().filter_map(Value::as_str).collect(),
        Value::Object(events) => events.keys().map(String::as_str).collect(),
        _ => return None,
    };
    let dispatch = events
        .iter()
        .any(|event| event.eq_ignore_ascii_case("workflow_dispatch"));
    let call = events
        .iter()
        .any(|event| event.eq_ignore_ascii_case("workflow_call"));
    if !dispatch && !call {
        return None;
    }
    let mut names = BTreeSet::new();
    for (present, event) in [(dispatch, "workflow_dispatch"), (call, "workflow_call")] {
        if !present {
            continue;
        }
        let Some(declared) = on.get(event).and_then(|config| config.get("inputs")) else {
            continue;
        };
        match declared {
            Value::Object(declared) => {
                for key in declared.keys() {
                    if key.contains("${{") {
                        return None;
                    }
                    names.insert(key.to_ascii_lowercase());
                }
            }
            Value::Null => {}
            _ => return None,
        }
    }
    Some(names)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    value: &Value,
    pointer: &str,
    job: Option<&JobFacts>,
    step: Option<usize>,
    in_steps: bool,
    disabled: bool,
    index: &Index,
    findings: &mut Vec<Diagnostic>,
) {
    match value {
        Value::Object(object) => {
            let entering_job = pointer == "/jobs";
            for (key, child) in object {
                let child_pointer = child_pointer(pointer, key);
                let (child_job, child_step, child_in_steps) = if entering_job {
                    (index.jobs.get(&key.to_ascii_lowercase()), None, false)
                } else if key == "steps" && at_job_object(pointer) {
                    (job, None, true)
                } else {
                    (job, step, false)
                };
                let child_disabled = disabled || (entering_job && is_disabled(child));
                walk(
                    child,
                    &child_pointer,
                    child_job,
                    child_step,
                    child_in_steps,
                    child_disabled,
                    index,
                    findings,
                );
            }
        }
        Value::Array(array) => {
            for (position, child) in array.iter().enumerate() {
                let child_pointer = format!("{pointer}/{position}");
                if in_steps {
                    walk(
                        child,
                        &child_pointer,
                        job,
                        Some(position),
                        false,
                        disabled || is_disabled(child),
                        index,
                        findings,
                    );
                } else {
                    walk(
                        child,
                        &child_pointer,
                        job,
                        step,
                        false,
                        disabled,
                        index,
                        findings,
                    );
                }
            }
        }
        Value::String(text) if !disabled => {
            check_text(text, pointer, job, step, index, findings);
        }
        _ => {}
    }
}

fn check_text(
    text: &str,
    pointer: &str,
    job: Option<&JobFacts>,
    step: Option<usize>,
    index: &Index,
    findings: &mut Vec<Diagnostic>,
) {
    let condition = condition(pointer);
    let bodies: Vec<&str> = if condition && !text.contains("${{") {
        vec![text]
    } else {
        expressions(text)
    };
    for body in bodies {
        let Ok(paths) = collect_context_paths(body) else {
            // The syntax pass reports unparsable expressions.
            continue;
        };
        for path in &paths {
            check_path(path, pointer, job, step, index, findings);
        }
    }
}

fn check_path(
    path: &preloop_gha_expressions::ContextPath,
    pointer: &str,
    job: Option<&JobFacts>,
    step: Option<usize>,
    index: &Index,
    findings: &mut Vec<Diagnostic>,
) {
    let context = path.context.to_ascii_lowercase();
    if !KNOWN_CONTEXTS.contains(&context.as_str()) {
        findings.push(warning(
            "preloop/unknown-context-property",
            pointer,
            format!(
                "`{}` is not a GitHub Actions context name",
                path.context
            ),
            "context names are case-insensitive; check the spelling against the GitHub contexts reference",
        ));
        return;
    }
    if let Some(key) = availability(pointer)
        && !contexts(key).contains(&context.as_str())
    {
        findings.push(error(
            "preloop/unavailable-context",
            pointer,
            format!("`{context}` context is not available in `{key}`"),
            "move the expression to a workflow key where GitHub provides this context; see the context availability table",
        ));
    }
    match context.as_str() {
        "steps" => check_steps_reference(path, pointer, job, step, findings),
        "needs" => check_needs_reference(path, pointer, job, index, findings),
        "matrix" => check_matrix_reference(path, pointer, job, findings),
        "inputs" => check_inputs_reference(path, pointer, index, findings),
        _ => {}
    }
    check_closed_properties(&context, path, pointer, findings);
}

fn check_steps_reference(
    path: &preloop_gha_expressions::ContextPath,
    pointer: &str,
    job: Option<&JobFacts>,
    step: Option<usize>,
    findings: &mut Vec<Diagnostic>,
) {
    let Some(job) = job else {
        return;
    };
    let Some(id) = property(path, 0) else {
        return;
    };
    let Some(ids) = &job.steps else {
        return;
    };
    let Some(position) = ids.get(&id.to_ascii_lowercase()) else {
        findings.push(error(
            "preloop/undefined-step-reference",
            pointer,
            format!("step id `{id}` is not defined in job `{}`", job.id),
            "reference a step id defined earlier in the same job, or correct the id",
        ));
        return;
    };
    if let Some(current) = step
        && *position >= current
    {
        findings.push(error(
            "preloop/undefined-step-reference",
            pointer,
            format!(
                "step id `{id}` runs at or after this step in job `{}`",
                job.id
            ),
            "a step can only read outputs from steps that run before it",
        ));
    }
}

fn check_needs_reference(
    path: &preloop_gha_expressions::ContextPath,
    pointer: &str,
    job: Option<&JobFacts>,
    index: &Index,
    findings: &mut Vec<Diagnostic>,
) {
    let Some(job) = job else {
        return;
    };
    let Some(need) = property(path, 0) else {
        return;
    };
    let Some(needs) = &job.needs else {
        return;
    };
    let need = need.to_ascii_lowercase();
    if !needs.contains(&need) {
        findings.push(error(
            "preloop/undefined-needs-reference",
            pointer,
            format!(
                "job `{}` reads `needs.{need}`, but `{need}` is not in its `needs:`",
                job.id
            ),
            "add the job to `needs:` or correct the job id",
        ));
        return;
    }
    if property(path, 1).is_some_and(|segment| segment.eq_ignore_ascii_case("outputs"))
        && let Some(output) = property(path, 2)
        && let Some(outputs) = index
            .jobs
            .get(&need)
            .and_then(|needed| needed.outputs.as_ref())
        && !outputs.contains(&output.to_ascii_lowercase())
    {
        findings.push(error(
            "preloop/undefined-needs-reference",
            pointer,
            format!(
                "job `{}` reads output `{output}` from `needs.{need}`, but job `{need}` declares no such output",
                job.id
            ),
            "read a declared output, or add it to the job's `outputs:` mapping",
        ));
    }
}

fn check_matrix_reference(
    path: &preloop_gha_expressions::ContextPath,
    pointer: &str,
    job: Option<&JobFacts>,
    findings: &mut Vec<Diagnostic>,
) {
    let Some(job) = job else {
        return;
    };
    if !job.matrix.present {
        findings.push(error(
            "preloop/undefined-matrix-key",
            pointer,
            format!(
                "`matrix` is used in job `{}`, which has no `strategy.matrix`",
                job.id
            ),
            "add a matrix to the job or remove the matrix reference",
        ));
        return;
    }
    let Some(keys) = &job.matrix.keys else {
        return;
    };
    let Some(key) = property(path, 0) else {
        return;
    };
    if !keys.contains(&key.to_ascii_lowercase()) {
        findings.push(error(
            "preloop/undefined-matrix-key",
            pointer,
            format!(
                "matrix key `{key}` is not defined by job `{}`'s `strategy.matrix`",
                job.id
            ),
            "use an axis name or a key defined by an `include` entry",
        ));
    }
}

fn check_inputs_reference(
    path: &preloop_gha_expressions::ContextPath,
    pointer: &str,
    index: &Index,
    findings: &mut Vec<Diagnostic>,
) {
    let Some(inputs) = &index.inputs else {
        return;
    };
    let Some(name) = property(path, 0) else {
        return;
    };
    if !inputs.contains(&name.to_ascii_lowercase()) {
        findings.push(error(
            "preloop/undefined-input",
            pointer,
            format!(
                "input `{name}` is not declared by `workflow_dispatch.inputs` or `workflow_call.inputs`"
            ),
            "declare the input on the triggering event or correct its name",
        ));
    }
}

fn check_closed_properties(
    context: &str,
    path: &preloop_gha_expressions::ContextPath,
    pointer: &str,
    findings: &mut Vec<Diagnostic>,
) {
    match context {
        "github" => {
            if let Some(property) = property(path, 0) {
                unknown_property(
                    context,
                    path,
                    property,
                    0,
                    pointer,
                    findings,
                    GITHUB_PROPERTIES,
                );
            }
        }
        "runner" => {
            if let Some(property) = property(path, 0) {
                unknown_property(
                    context,
                    path,
                    property,
                    0,
                    pointer,
                    findings,
                    RUNNER_PROPERTIES,
                );
            }
        }
        "strategy" => {
            if let Some(property) = property(path, 0) {
                unknown_property(
                    context,
                    path,
                    property,
                    0,
                    pointer,
                    findings,
                    STRATEGY_PROPERTIES,
                );
            }
        }
        "job" => {
            let Some(first) = property(path, 0) else {
                return;
            };
            if !contains_ignore_ascii_case(JOB_PROPERTIES, first) {
                unknown_property(context, path, first, 0, pointer, findings, JOB_PROPERTIES);
                return;
            }
            if first.eq_ignore_ascii_case("container") {
                if let Some(property) = property(path, 1) {
                    unknown_property(
                        context,
                        path,
                        property,
                        1,
                        pointer,
                        findings,
                        JOB_CONTAINER_PROPERTIES,
                    );
                }
            } else if first.eq_ignore_ascii_case("services")
                && let Some(property) = property(path, 2)
            {
                unknown_property(
                    context,
                    path,
                    property,
                    2,
                    pointer,
                    findings,
                    JOB_SERVICE_PROPERTIES,
                );
            }
        }
        _ => {}
    }
}

fn unknown_property(
    context: &str,
    path: &preloop_gha_expressions::ContextPath,
    property: &str,
    position: usize,
    pointer: &str,
    findings: &mut Vec<Diagnostic>,
    known: &[&str],
) {
    if contains_ignore_ascii_case(known, property) {
        return;
    }
    let full = path.properties[..=position]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(".");
    findings.push(warning(
        "preloop/unknown-context-property",
        pointer,
        format!("`{context}.{full}` is not a property of the `{context}` context"),
        "check the property spelling against the GitHub contexts reference; property names are case-insensitive",
    ));
}

/// The literal property at `position`, or `None` for a missing or `*` segment.
fn property(path: &preloop_gha_expressions::ContextPath, position: usize) -> Option<&str> {
    path.properties
        .get(position)
        .map(String::as_str)
        .filter(|segment| *segment != "*")
}

fn contains_ignore_ascii_case(values: &[&str], value: &str) -> bool {
    values.iter().any(|known| known.eq_ignore_ascii_case(value))
}

fn condition(pointer: &str) -> bool {
    let mut segments = pointer.split('/');
    if segments.next() != Some("") {
        return false;
    }
    let segments: Vec<&str> = segments.collect();
    matches!(
        segments.as_slice(),
        ["jobs", _, "if"] | ["jobs", _, "steps", _, "if"] | ["runs", "steps", _, "if"]
    )
}

fn at_job_object(pointer: &str) -> bool {
    let mut segments = pointer.split('/');
    segments.next() == Some("")
        && segments.next() == Some("jobs")
        && segments.next().is_some()
        && segments.next().is_none()
}

fn child_pointer(pointer: &str, key: &str) -> String {
    format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// The GitHub context-availability key for a JSON Pointer, or `None` when the
/// table does not constrain that key (all contexts are available).
fn availability(pointer: &str) -> Option<&'static str> {
    let mut segments = pointer.split('/');
    if segments.next() != Some("") {
        return None;
    }
    let segments: Vec<&str> = segments.collect();
    match segments.as_slice() {
        ["env", ..] => Some("env"),
        ["concurrency", ..] => Some("concurrency"),
        ["run-name"] => Some("run-name"),
        ["on", "workflow_call", "inputs", _, "default"] => {
            Some("on.workflow_call.inputs.<inputs_id>.default")
        }
        ["on", "workflow_call", "outputs", _, "value"] => {
            Some("on.workflow_call.outputs.<output_id>.value")
        }
        ["jobs", _, "if"] => Some("jobs.<job_id>.if"),
        ["jobs", _, "runs-on", ..] => Some("jobs.<job_id>.runs-on"),
        ["jobs", _, "concurrency", ..] => Some("jobs.<job_id>.concurrency"),
        ["jobs", _, "container"] => Some("jobs.<job_id>.container"),
        ["jobs", _, "container", "image"] => Some("jobs.<job_id>.container.image"),
        ["jobs", _, "container", "credentials", ..] => Some("jobs.<job_id>.container.credentials"),
        ["jobs", _, "container", "env", ..] => Some("jobs.<job_id>.container.env.<env_id>"),
        ["jobs", _, "continue-on-error"] => Some("jobs.<job_id>.continue-on-error"),
        ["jobs", _, "defaults", "run", ..] => Some("jobs.<job_id>.defaults.run"),
        ["jobs", _, "env", ..] => Some("jobs.<job_id>.env"),
        ["jobs", _, "environment", "url"] => Some("jobs.<job_id>.environment.url"),
        ["jobs", _, "environment"] | ["jobs", _, "environment", "name"] => {
            Some("jobs.<job_id>.environment")
        }
        ["jobs", _, "name"] => Some("jobs.<job_id>.name"),
        ["jobs", _, "outputs", _] => Some("jobs.<job_id>.outputs.<output_id>"),
        ["jobs", _, "secrets", _] => Some("jobs.<job_id>.secrets.<secrets_id>"),
        ["jobs", _, "services"] => Some("jobs.<job_id>.services"),
        ["jobs", _, "services", _, "credentials", ..] => {
            Some("jobs.<job_id>.services.<service_id>.credentials")
        }
        ["jobs", _, "services", _, "env", ..] => {
            Some("jobs.<job_id>.services.<service_id>.env.<env_id>")
        }
        ["jobs", _, "snapshot", "if"] => Some("jobs.<job_id>.snapshot.if"),
        ["jobs", _, "strategy", ..] => Some("jobs.<job_id>.strategy"),
        ["jobs", _, "timeout-minutes"] => Some("jobs.<job_id>.timeout-minutes"),
        ["jobs", _, "with", _] => Some("jobs.<job_id>.with.<with_id>"),
        ["jobs", _, "steps", _, "continue-on-error"] => {
            Some("jobs.<job_id>.steps.continue-on-error")
        }
        ["jobs", _, "steps", _, "env", ..] => Some("jobs.<job_id>.steps.env"),
        ["jobs", _, "steps", _, "if"] => Some("jobs.<job_id>.steps.if"),
        ["jobs", _, "steps", _, "name"] => Some("jobs.<job_id>.steps.name"),
        ["jobs", _, "steps", _, "run"] => Some("jobs.<job_id>.steps.run"),
        ["jobs", _, "steps", _, "timeout-minutes"] => Some("jobs.<job_id>.steps.timeout-minutes"),
        ["jobs", _, "steps", _, "with", ..] => Some("jobs.<job_id>.steps.with"),
        ["jobs", _, "steps", _, "working-directory"] => {
            Some("jobs.<job_id>.steps.working-directory")
        }
        _ => None,
    }
}

fn contexts(key: &str) -> &'static [&'static str] {
    match key {
        "jobs.<job_id>.outputs.<output_id>"
        | "jobs.<job_id>.steps.continue-on-error"
        | "jobs.<job_id>.steps.env"
        | "jobs.<job_id>.steps.name"
        | "jobs.<job_id>.steps.run"
        | "jobs.<job_id>.steps.timeout-minutes"
        | "jobs.<job_id>.steps.with"
        | "jobs.<job_id>.steps.working-directory" => EVERYTHING,
        "jobs.<job_id>.container.env.<env_id>"
        | "jobs.<job_id>.services.<service_id>.env.<env_id>" => WITHOUT_STEPS,
        "jobs.<job_id>.environment.url" | "jobs.<job_id>.steps.if" => WITHOUT_SECRETS,
        "jobs.<job_id>.container.credentials"
        | "jobs.<job_id>.services.<service_id>.credentials" => WITHOUT_JOB_RUNNER_STEPS,
        "jobs.<job_id>.defaults.run" => DEFAULTS_RUN,
        "on.workflow_call.outputs.<output_id>.value" => WORKFLOW_CALL_OUTPUT,
        "jobs.<job_id>.env" | "jobs.<job_id>.secrets.<secrets_id>" => JOB_ENV,
        "jobs.<job_id>.concurrency"
        | "jobs.<job_id>.container"
        | "jobs.<job_id>.container.image"
        | "jobs.<job_id>.continue-on-error"
        | "jobs.<job_id>.environment"
        | "jobs.<job_id>.name"
        | "jobs.<job_id>.runs-on"
        | "jobs.<job_id>.services"
        | "jobs.<job_id>.timeout-minutes"
        | "jobs.<job_id>.with.<with_id>"
        | "jobs.<job_id>.snapshot.if" => JOB_SCALAR,
        "jobs.<job_id>.strategy" | "jobs.<job_id>.if" => JOB_WITHOUT_MATRIX,
        "env" => WORKFLOW_ENV,
        "concurrency" | "on.workflow_call.inputs.<inputs_id>.default" | "run-name" => {
            WORKFLOW_SCALAR
        }
        _ => EVERYTHING,
    }
}

fn error(rule_id: &str, path: &str, message: impl Into<String>, help: &str) -> Diagnostic {
    let mut finding = diagnostic(rule_id, path, message.into(), help);
    finding.severity = Severity::Error;
    finding.confidence = Confidence::High;
    finding
}

fn warning(rule_id: &str, path: &str, message: impl Into<String>, help: &str) -> Diagnostic {
    diagnostic(rule_id, path, message.into(), help)
}

#[cfg(test)]
mod tests {
    use preloop_gha_authoring::WorkflowDocument;
    use serde_json::{Value, json};

    use crate::{LintContext, lint_workflow};

    fn paths(value: Value, rule: &str) -> Vec<String> {
        let document = WorkflowDocument::from_value(value).expect("workflow document");
        lint_workflow(&document, &LintContext::default())
            .into_iter()
            .filter(|finding| finding.rule_id == rule)
            .map(|finding| finding.path)
            .collect()
    }

    fn workflow(jobs: Value) -> Value {
        json!({"name": "ci", "on": {"push": {}}, "jobs": jobs})
    }

    fn run_job(steps: Value) -> Value {
        json!({"runs-on": "ubuntu-latest", "steps": steps})
    }

    #[test]
    fn undefined_step_reference_reports_missing_and_forward_ids() {
        let found = paths(
            workflow(json!({"build": run_job(json!([
                {"id": "first", "run": "${{ steps.second.outputs.x }}"},
                {"id": "second", "run": "echo ${{ steps.missing.outputs.y }}"}
            ]))})),
            "preloop/undefined-step-reference",
        );
        assert_eq!(
            found,
            vec!["/jobs/build/steps/0/run", "/jobs/build/steps/1/run"]
        );
    }

    #[test]
    fn undefined_step_reference_accepts_earlier_ids_case_insensitively() {
        let found = paths(
            workflow(json!({"build": run_job(json!([
                {"id": "Build", "run": "echo built"},
                {"run": "echo ${{ STEPS.build.outputs.artifact }}"}
            ]))})),
            "preloop/undefined-step-reference",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn undefined_step_reference_skips_dynamic_access() {
        let found = paths(
            workflow(json!({"build": run_job(json!([
                {"id": "first", "run": "echo built"},
                {"run": "${{ steps[format('{0}', 'first')].outputs.x }}"}
            ]))})),
            "preloop/undefined-step-reference",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn undefined_needs_reference_reports_undeclared_jobs_and_outputs() {
        let found = paths(
            workflow(json!({
                "build": {
                    "runs-on": "ubuntu-latest",
                    "outputs": {"tag": "${{ steps.make.outputs.tag }}"},
                    "steps": [{"id": "make", "run": "echo tag=1"}]
                },
                "deploy": {
                    "needs": "build",
                    "runs-on": "ubuntu-latest",
                    "steps": [{"run": "echo ${{ needs.build.outputs.missing }}"}]
                },
                "other": {
                    "needs": "build",
                    "runs-on": "ubuntu-latest",
                    "steps": [{"run": "echo ${{ needs.deploy.outputs.tag }}"}]
                }
            })),
            "preloop/undefined-needs-reference",
        );
        assert_eq!(
            found,
            vec!["/jobs/deploy/steps/0/run", "/jobs/other/steps/0/run"]
        );
    }

    #[test]
    fn undefined_needs_reference_accepts_declared_jobs_and_outputs() {
        let found = paths(
            workflow(json!({
                "build": {
                    "runs-on": "ubuntu-latest",
                    "outputs": {"tag": "${{ steps.make.outputs.tag }}"},
                    "steps": [{"id": "make", "run": "echo tag=1"}]
                },
                "deploy": {
                    "needs": ["build"],
                    "runs-on": "ubuntu-latest",
                    "steps": [{"run": "echo ${{ needs.build.outputs.tag }} ${{ needs.build.result }}"}]
                }
            })),
            "preloop/undefined-needs-reference",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn undefined_needs_reference_skips_dynamic_access() {
        let found = paths(
            workflow(json!({
                "build": {"runs-on": "ubuntu-latest", "steps": [{"run": "echo hi"}]},
                "deploy": {
                    "needs": "build",
                    "runs-on": "ubuntu-latest",
                    "steps": [{"run": "${{ needs[format('{0}', 'build')].outputs.tag }}"}]
                }
            })),
            "preloop/undefined-needs-reference",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn undefined_matrix_key_reports_unknown_axis() {
        let found = paths(
            workflow(json!({"build": {
                "runs-on": "ubuntu-latest",
                "strategy": {"matrix": {"os": ["ubuntu-latest"]}},
                "steps": [{"run": "echo ${{ matrix.arch }} ${{ matrix.os }}"}]
            }})),
            "preloop/undefined-matrix-key",
        );
        assert_eq!(found, vec!["/jobs/build/steps/0/run"]);
    }

    #[test]
    fn undefined_matrix_key_accepts_include_keys() {
        let found = paths(
            workflow(json!({"build": {
                "runs-on": "ubuntu-latest",
                "strategy": {"matrix": {"include": [{"arch": "x86_64"}]}},
                "steps": [{"run": "echo ${{ matrix.arch }}"}]
            }})),
            "preloop/undefined-matrix-key",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn undefined_matrix_key_reports_jobs_without_a_matrix() {
        let found = paths(
            workflow(json!({"build": run_job(json!([
                {"run": "echo ${{ matrix.os }}"}
            ]))})),
            "preloop/undefined-matrix-key",
        );
        assert_eq!(found, vec!["/jobs/build/steps/0/run"]);
    }

    #[test]
    fn undefined_matrix_key_skips_expression_matrices_and_dynamic_access() {
        let found = paths(
            workflow(json!({"build": {
                "runs-on": "ubuntu-latest",
                "strategy": {"matrix": "${{ fromJSON('{}') }}"},
                "steps": [{"run": "echo ${{ matrix.os }} ${{ matrix[format('{0}', 'os')] }}"}]
            }})),
            "preloop/undefined-matrix-key",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn undefined_input_reports_undeclared_names() {
        let found = paths(
            json!({
                "name": "ci",
                "on": {"workflow_dispatch": {"inputs": {"good": {"type": "string"}}}},
                "jobs": {"build": run_job(json!([
                    {"run": "echo ${{ inputs.bad }} ${{ inputs.good }}"}
                ]))}
            }),
            "preloop/undefined-input",
        );
        assert_eq!(found, vec!["/jobs/build/steps/0/run"]);
    }

    #[test]
    fn undefined_input_reports_workflow_call_inputs_and_empty_declarations() {
        let found = paths(
            json!({
                "name": "ci",
                "on": {"workflow_call": {}, "workflow_dispatch": {"inputs": {"name": {"type": "string"}}}},
                "jobs": {"build": run_job(json!([
                    {"run": "echo ${{ inputs.name }} ${{ inputs.other }}"}
                ]))}
            }),
            "preloop/undefined-input",
        );
        assert_eq!(found, vec!["/jobs/build/steps/0/run"]);
    }

    #[test]
    fn undefined_input_skips_workflows_without_dispatch_or_call_events() {
        let found = paths(
            workflow(json!({"build": run_job(json!([
                {"run": "echo ${{ inputs.maybe }}"}
            ]))})),
            "preloop/undefined-input",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn undefined_input_skips_dynamic_access() {
        let found = paths(
            json!({
                "name": "ci",
                "on": {"workflow_dispatch": {"inputs": {"good": {"type": "string"}}}},
                "jobs": {"build": run_job(json!([
                    {"run": "${{ inputs[format('{0}', 'good')] }}"}
                ]))}
            }),
            "preloop/undefined-input",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn unavailable_context_reports_contexts_outside_the_availability_table() {
        let found = paths(
            json!({
                "name": "ci",
                "on": {"push": {}},
                "concurrency": {"group": "${{ env.GROUP }}"},
                "jobs": {
                    "build": {
                        "runs-on": "${{ steps.build.outputs.runner }}",
                        "if": "${{ secrets.TOKEN != '' }}",
                        "steps": [{"id": "build", "run": "echo hi"}]
                    }
                }
            }),
            "preloop/unavailable-context",
        );
        assert_eq!(
            found,
            vec![
                "/concurrency/group",
                "/jobs/build/if",
                "/jobs/build/runs-on"
            ]
        );
    }

    #[test]
    fn unavailable_context_accepts_matrix_in_runs_on() {
        let found = paths(
            workflow(json!({"build": {
                "runs-on": "${{ matrix.os }}",
                "strategy": {"matrix": {"os": ["ubuntu-latest"]}},
                "steps": [{"run": "echo hi"}]
            }})),
            "preloop/unavailable-context",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn unknown_context_property_reports_closed_contexts() {
        let found = paths(
            workflow(json!({"build": run_job(json!([
                {"run": "echo ${{ github.events }} ${{ runner.version }} ${{ bogus.value }}"}
            ]))})),
            "preloop/unknown-context-property",
        );
        assert_eq!(found, vec!["/jobs/build/steps/0/run"; 3]);
    }

    #[test]
    fn unknown_context_property_leaves_open_and_dynamic_access_alone() {
        let found = paths(
            workflow(json!({"build": run_job(json!([
                {"run": "echo ${{ github.event.issue.title }} ${{ strategy.fail-fast }}"},
                {"run": "${{ github[format('{0}', 'event')] }} ${{ job.services.redis.ports[6379] }}"}
            ]))})),
            "preloop/unknown-context-property",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn disabled_steps_skip_reference_checks() {
        let found = paths(
            workflow(json!({"build": run_job(json!([
                {"if": false, "run": "${{ steps.missing.outputs.x }} ${{ matrix.os }}"}
            ]))})),
            "preloop/undefined-step-reference",
        );
        assert!(found.is_empty(), "{found:?}");
    }
}

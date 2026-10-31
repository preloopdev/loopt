//! Structural correctness checks that do not require evaluating expressions.
//!
//! Unknown fields are deliberately preserved by the authoring model, so these
//! rules only reject shapes GitHub itself cannot execute. Any value containing
//! an expression is treated as unknown rather than invalid.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::{Confidence, Diagnostic, Severity};

pub(crate) fn check(doc: &Value, findings: &mut Vec<Diagnostic>) {
    if let Some(jobs) = doc.get("jobs").and_then(Value::as_object) {
        check_jobs(jobs, findings);
    }
    if let Some(steps) = doc
        .get("runs")
        .and_then(|runs| runs.get("steps"))
        .and_then(Value::as_array)
    {
        check_steps(steps, "/runs/steps", findings);
    }
}

fn check_jobs(jobs: &serde_json::Map<String, Value>, findings: &mut Vec<Diagnostic>) {
    // GitHub job ids and `needs:` references match case-insensitively.
    let known: BTreeSet<String> = jobs.keys().map(|id| id.to_ascii_lowercase()).collect();

    for (job_id, job) in jobs {
        let Some(job) = job.as_object() else {
            continue;
        };
        let path = format!("/jobs/{}", escape(job_id));
        let has_runs = job.get("runs-on").is_some();
        let has_uses = job.get("uses").is_some();
        let has_steps = job
            .get("steps")
            .and_then(Value::as_array)
            .is_some_and(|steps| !steps.is_empty());

        if has_uses {
            for forbidden in ["runs-on", "steps", "container", "services"] {
                if job.contains_key(forbidden) {
                    findings.push(error(
                        "preloop/job-uses-conflict",
                        &format!("{path}/{}", escape(forbidden)),
                        format!(
                            "job `{job_id}` calls a reusable workflow with `uses:` and also sets `{forbidden}`, which GitHub rejects"
                        ),
                        "remove the field from this job; reusable-call jobs cannot define it",
                    ));
                }
            }
        } else if !has_runs {
            findings.push(error(
                "preloop/job-required-target",
                &path,
                format!("job `{job_id}` has neither `runs-on:` nor `uses:`"),
                "add a runner selection or call a reusable workflow",
            ));
        } else if !has_steps {
            findings.push(error(
                "preloop/job-required-target",
                &format!("{path}/steps"),
                format!("job `{job_id}` has no steps"),
                "add at least one step, or call a reusable workflow with `uses:`",
            ));
        }

        if let Some(steps) = job.get("steps").and_then(Value::as_array) {
            check_steps(steps, &format!("{path}/steps"), findings);
        }
        check_needs(job_id, job, &path, &known, findings);
        check_timeout(
            job.get("timeout-minutes"),
            &format!("{path}/timeout-minutes"),
            findings,
        );
        check_matrix(job, &path, findings);
    }

    check_cycles(jobs, findings);
}

fn check_steps(steps: &[Value], path: &str, findings: &mut Vec<Diagnostic>) {
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, step) in steps.iter().enumerate() {
        let Some(step) = step.as_object() else {
            continue;
        };
        let step_path = format!("{path}/{index}");
        let has_run = step.get("run").is_some();
        let has_uses = step.get("uses").is_some();
        if has_run == has_uses {
            findings.push(error(
                "preloop/step-required-target",
                &step_path,
                "step must define exactly one of `run:` or `uses:`",
                "a step either runs a script or calls an action",
            ));
        }
        if has_uses {
            for forbidden in ["shell", "working-directory"] {
                if step.contains_key(forbidden) {
                    findings.push(warning(
                        "preloop/step-uses-conflict",
                        &format!("{step_path}/{}", escape(forbidden)),
                        format!("`{forbidden}` only applies to `run:` steps, not actions"),
                        "remove it from the `uses:` step",
                    ));
                }
            }
        }
        if let Some(id) = step.get("id").and_then(Value::as_str)
            && ids.insert(id, index).is_some()
        {
            findings.push(error(
                "preloop/duplicate-step-id",
                &format!("{step_path}/id"),
                format!("step id `{id}` is used more than once in this job"),
                "step ids must be unique so `steps.{id}.outputs` is unambiguous",
            ));
        }
        if let Some(timeout) = step.get("timeout-minutes") {
            check_timeout(
                Some(timeout),
                &format!("{step_path}/timeout-minutes"),
                findings,
            );
        }
    }
}

fn check_needs(
    job_id: &str,
    job: &serde_json::Map<String, Value>,
    path: &str,
    known: &BTreeSet<String>,
    findings: &mut Vec<Diagnostic>,
) {
    let Some(needs) = job.get("needs") else {
        return;
    };
    let entries: Vec<&str> = match needs {
        Value::String(need) => vec![need.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => return,
    };
    for need in entries {
        if need.contains("${{") {
            continue;
        }
        if !known.contains(&need.to_ascii_lowercase()) {
            findings.push(error(
                "preloop/undefined-need",
                &format!("{path}/needs"),
                format!("job `{job_id}` needs `{need}`, which is not defined in `jobs:`"),
                "correct the job id or add the missing job",
            ));
        }
    }
}

fn check_cycles(jobs: &serde_json::Map<String, Value>, findings: &mut Vec<Diagnostic>) {
    let known: BTreeSet<String> = jobs.keys().map(|id| id.to_ascii_lowercase()).collect();
    let graph: BTreeMap<String, Vec<String>> = jobs
        .iter()
        .map(|(id, job)| {
            let needs = job
                .get("needs")
                .map(|needs| match needs {
                    Value::String(need) => vec![need.as_str()],
                    Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
                    _ => Vec::new(),
                })
                .unwrap_or_default()
                .into_iter()
                .map(str::to_ascii_lowercase)
                .filter(|need| known.contains(need))
                .collect();
            (id.to_ascii_lowercase(), needs)
        })
        .collect();

    let mut visiting = BTreeSet::new();
    let mut done = BTreeSet::new();
    let mut reported = BTreeSet::new();
    for job in graph.keys() {
        visit(
            job,
            &graph,
            &mut visiting,
            &mut done,
            &mut reported,
            findings,
        );
    }
}

fn visit(
    job: &str,
    graph: &BTreeMap<String, Vec<String>>,
    visiting: &mut BTreeSet<String>,
    done: &mut BTreeSet<String>,
    reported: &mut BTreeSet<String>,
    findings: &mut Vec<Diagnostic>,
) {
    if done.contains(job) {
        return;
    }
    let first_visit = visiting.insert(job.to_owned());
    if !first_visit && reported.insert(job.to_owned()) {
        findings.push(error(
            "preloop/needs-cycle",
            &format!("/jobs/{}/needs", escape(job)),
            format!("job `{job}` is part of a `needs:` cycle"),
            "remove one dependency edge so the job graph is acyclic",
        ));
    }
    if !first_visit {
        return;
    }

    if let Some(needs) = graph.get(job) {
        for need in needs {
            visit(need, graph, visiting, done, reported, findings);
        }
    }
    visiting.remove(job);
    done.insert(job.to_owned());
}

fn check_timeout(value: Option<&Value>, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::Number(number) if number.as_u64().is_some_and(|minutes| minutes > 0) => {}
        Value::String(text) if text.contains("${{") => {}
        _ => findings.push(error(
            "preloop/invalid-timeout",
            path,
            "`timeout-minutes` must be a positive integer or an expression",
            "use a positive whole number of minutes",
        )),
    }
}

fn check_matrix(job: &serde_json::Map<String, Value>, path: &str, findings: &mut Vec<Diagnostic>) {
    let Some(matrix) = job
        .get("strategy")
        .and_then(|strategy| strategy.get("matrix"))
    else {
        return;
    };
    let matrix_path = format!("{path}/strategy/matrix");
    let Some(matrix) = matrix.as_object() else {
        if matrix.as_str().is_some_and(|text| text.contains("${{")) {
            return;
        }
        findings.push(error(
            "preloop/invalid-matrix",
            &matrix_path,
            "`strategy.matrix` must be a mapping or an expression",
            "define axes as mapping keys, or use `${{ fromJSON(...) }}`",
        ));
        return;
    };

    for (axis, values) in matrix {
        let axis_path = format!("{matrix_path}/{}", escape(axis));
        if axis == "include" || axis == "exclude" {
            if values.as_str().is_some_and(|text| text.contains("${{")) {
                continue;
            }
            let Some(entries) = values.as_array() else {
                findings.push(error(
                    "preloop/invalid-matrix",
                    &axis_path,
                    format!("matrix `{axis}` must be an array of objects"),
                    "write each include/exclude entry as a mapping",
                ));
                continue;
            };
            if entries.iter().any(|entry| !entry.is_object()) {
                findings.push(error(
                    "preloop/invalid-matrix",
                    &axis_path,
                    format!("matrix `{axis}` entries must be objects"),
                    "write each include/exclude entry as a mapping",
                ));
            }
            continue;
        }
        if values.as_str().is_some_and(|text| text.contains("${{")) {
            continue;
        }
        let Some(items) = values.as_array() else {
            findings.push(error(
                "preloop/invalid-matrix",
                &axis_path,
                format!("matrix axis `{axis}` must be an array or an expression"),
                "write the axis as a list of values",
            ));
            continue;
        };
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for item in items {
            let key = serde_json::to_string(item).unwrap_or_default();
            if !seen.insert(key) {
                findings.push(warning(
                    "preloop/duplicate-matrix-value",
                    &axis_path,
                    format!("matrix axis `{axis}` repeats a value"),
                    "remove the duplicate; it produces an indistinguishable extra job",
                ));
            }
        }
    }
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

fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

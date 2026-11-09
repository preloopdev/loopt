//! Runner label checks for `runs-on`.
//!
//! Tables mirror:
//! - GitHub-hosted labels:
//!   <https://docs.github.com/en/actions/using-github-hosted-runners/about-github-hosted-runners>
//! - preset self-hosted labels:
//!   <https://docs.github.com/en/actions/hosting-your-own-runners/managing-self-hosted-runners/using-self-hosted-runners-in-a-workflow#using-default-labels-to-route-jobs>
//!
//! A supplied runner inventory is authoritative and is left to the existing
//! `unsatisfied-runner` check. Without one, a label that is neither hosted nor
//! a preset self-hosted label is only an advisory: it may be a custom label.

use serde_json::Value;

use crate::{Confidence, Diagnostic, LintContext, Severity, diagnostic, is_disabled};

/// Current GitHub-hosted images, plus the documented larger-runner labels.
const GITHUB_HOSTED_LABELS: &[&str] = &[
    "macos-14",
    "macos-14-large",
    "macos-14-xlarge",
    "macos-15",
    "macos-15-intel",
    "macos-15-large",
    "macos-15-xlarge",
    "macos-26",
    "macos-26-intel",
    "macos-26-large",
    "macos-26-xlarge",
    "macos-latest",
    "macos-latest-large",
    "macos-latest-xlarge",
    "ubuntu-22.04",
    "ubuntu-22.04-arm",
    "ubuntu-24.04",
    "ubuntu-24.04-arm",
    "ubuntu-latest",
    "ubuntu-latest-16-cores",
    "ubuntu-latest-4-cores",
    "ubuntu-latest-8-cores",
    "ubuntu-slim",
    "windows-11-arm",
    "windows-2022",
    "windows-2025",
    "windows-2025-vs2026",
    "windows-latest",
    "windows-latest-8-cores",
];

/// Labels every self-hosted runner carries unless the default labels were
/// replaced.
const SELF_HOSTED_LABELS: &[&str] = &[
    "arm",
    "arm64",
    "linux",
    "macos",
    "self-hosted",
    "windows",
    "x64",
];

pub(crate) fn check(doc: &Value, context: &LintContext, findings: &mut Vec<Diagnostic>) {
    // A supplied inventory is authoritative; `unsatisfied-runner` owns it.
    if context.runner_label_sets.is_some() {
        return;
    }
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, value) in jobs {
        let Some(job) = value.as_object() else {
            continue;
        };
        // Reusable-call jobs run on the called workflow's runner, and disabled
        // jobs do not execute.
        if job.contains_key("uses") || is_disabled(value) {
            continue;
        }
        let Some(selector) = job.get("runs-on") else {
            continue;
        };
        let Some(labels) = literal_labels(selector) else {
            continue;
        };
        if labels
            .iter()
            .any(|label| label.contains("${{") || label.trim().eq_ignore_ascii_case("self-hosted"))
        {
            continue;
        }
        let indexed = selector.is_array();
        for (index, label) in labels.iter().enumerate() {
            let path = if indexed {
                format!("/jobs/{}/runs-on/{index}", escape(job_id))
            } else {
                format!("/jobs/{}/runs-on", escape(job_id))
            };
            check_label(label, &path, findings);
        }
    }
}

fn literal_labels(selector: &Value) -> Option<Vec<&str>> {
    match selector {
        Value::String(label) => Some(vec![label.as_str()]),
        Value::Array(labels) => labels.iter().map(Value::as_str).collect(),
        // `group:` selectors and non-literal shapes are not label lists.
        _ => None,
    }
}

fn check_label(label: &str, path: &str, findings: &mut Vec<Diagnostic>) {
    let label = label.trim();
    if label.is_empty() || label.contains("${{") {
        return;
    }
    let lower = label.to_ascii_lowercase();
    if is_github_hosted(&lower) {
        if lower.ends_with("-latest") {
            let mut finding = diagnostic(
                "preloop/floating-runner-image",
                path,
                format!(
                    "`{label}` selects the newest GitHub-hosted image, which moves between OS versions"
                ),
                "pin a specific image label (for example `ubuntu-24.04`) when the workflow depends on the runner image",
            );
            finding.severity = Severity::Info;
            finding.confidence = Confidence::High;
            findings.push(finding);
        }
        return;
    }
    if SELF_HOSTED_LABELS.iter().any(|known| lower == *known) {
        return;
    }
    findings.push(diagnostic(
        "preloop/unknown-runner-label",
        path,
        format!(
            "`{label}` is not a known GitHub-hosted or preset self-hosted runner label"
        ),
        "supply a runner inventory with `--context` for custom labels, or add the `self-hosted` label to select self-hosted runners",
    ));
}

fn is_github_hosted(label: &str) -> bool {
    GITHUB_HOSTED_LABELS.contains(&label) || is_larger_runner(label)
}

/// Documented larger runners use `-<cores>-core(s)` for Ubuntu and Windows and
/// `-large`/`-xlarge` for macOS.
fn is_larger_runner(label: &str) -> bool {
    if let Some(rest) = label
        .strip_prefix("ubuntu-")
        .or_else(|| label.strip_prefix("windows-"))
    {
        let parts: Vec<&str> = rest.split('-').collect();
        return match parts.as_slice() {
            [version, cores, "core" | "cores"] => {
                image_version(version) && cores.parse::<u32>().is_ok()
            }
            _ => false,
        };
    }
    if let Some(rest) = label.strip_prefix("macos-") {
        let parts: Vec<&str> = rest.split('-').collect();
        return match parts.as_slice() {
            [version, "large" | "xlarge"] => image_version(version),
            _ => false,
        };
    }
    false
}

fn image_version(version: &str) -> bool {
    version == "latest"
        || (!version.is_empty()
            && version.contains(|character: char| character.is_ascii_digit())
            && version
                .chars()
                .all(|character| character.is_ascii_digit() || character == '.'))
}

fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

//! Trigger validation for the workflow `on:` block.
//!
//! GitHub silently ignores or rejects trigger mistakes that look plausible:
//! misspelled events, activity types that do not exist for the event, filters
//! attached to events that do not support them, malformed filter globs, cron
//! expressions outside GitHub's subset, and malformed dispatch inputs. These
//! checks mirror actionlint's `events`, `glob`, and input rules while treating
//! any value containing an expression as unknown.

mod catalog;
mod cron;
mod glob;
mod inputs;

use serde_json::{Map, Value};

use crate::{Confidence, Diagnostic, Severity};

use catalog::ActivityTypes;

/// Inspect the workflow's `on:` value. All three GitHub spellings (a string, a
/// list of strings, and a mapping of event name to settings) are supported.
pub(crate) fn check(document: &Value, findings: &mut Vec<Diagnostic>) {
    let Some(on) = document.get("on") else {
        return;
    };
    match on {
        Value::String(event) => check_event_name(event, "/on", findings),
        Value::Array(events) => {
            for (index, event) in events.iter().enumerate() {
                if let Value::String(event) = event {
                    check_event_name(event, &format!("/on/{index}"), findings);
                }
            }
        }
        Value::Object(events) => {
            for (event, config) in events {
                check_event(event, config, &format!("/on/{}", escape(event)), findings);
            }
        }
        _ => {}
    }
}

fn check_event_name(event: &str, path: &str, findings: &mut Vec<Diagnostic>) {
    if event.contains("${{") {
        return;
    }
    if catalog::activity_types(event).is_none() {
        findings.push(unknown_event(event, path));
    }
}

fn check_event(event: &str, config: &Value, path: &str, findings: &mut Vec<Diagnostic>) {
    if event.contains("${{") {
        return;
    }
    let Some(activity) = catalog::activity_types(event) else {
        findings.push(unknown_event(event, path));
        return;
    };

    match event {
        "schedule" => cron::check_schedule(config, path, findings),
        "workflow_dispatch" => inputs::check_dispatch(config, path, findings),
        "workflow_call" => inputs::check_call(config, path, findings),
        _ => {}
    }

    let Some(config) = config.as_object() else {
        return;
    };
    if let Some(types) = config.get("types") {
        check_types(event, activity, types, path, findings);
    }
    check_filters(event, config, path, findings);
}

fn unknown_event(event: &str, path: &str) -> Diagnostic {
    error(
        "preloop/unknown-event",
        path,
        format!("`{event}` is not a GitHub workflow event"),
        "use an event from https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows",
    )
}

fn check_types(
    event: &str,
    activity: ActivityTypes,
    types: &Value,
    path: &str,
    findings: &mut Vec<Diagnostic>,
) {
    match types {
        Value::String(value) => {
            check_type(event, activity, value, &format!("{path}/types"), findings);
        }
        Value::Array(values) => {
            if values.is_empty() {
                return;
            }
            if matches!(activity, ActivityTypes::Listed([])) {
                findings.push(types_unsupported(event, &format!("{path}/types")));
                return;
            }
            for (index, value) in values.iter().enumerate() {
                if let Value::String(value) = value {
                    check_type(
                        event,
                        activity,
                        value,
                        &format!("{path}/types/{index}"),
                        findings,
                    );
                }
            }
        }
        _ => {}
    }
}

fn check_type(
    event: &str,
    activity: ActivityTypes,
    value: &str,
    path: &str,
    findings: &mut Vec<Diagnostic>,
) {
    if value.contains("${{") {
        return;
    }
    match activity {
        ActivityTypes::Any => {}
        ActivityTypes::Listed([]) => {
            findings.push(types_unsupported(event, path));
        }
        ActivityTypes::Listed(valid) => {
            if !valid.contains(&value) {
                findings.push(error(
                    "preloop/unknown-activity-type",
                    path,
                    format!("activity type `{value}` is not valid for the `{event}` event"),
                    &format!("valid types are {}", list(valid)),
                ));
            }
        }
    }
}

fn types_unsupported(event: &str, path: &str) -> Diagnostic {
    error(
        "preloop/unknown-activity-type",
        path,
        format!("`types` cannot be specified for the `{event}` event"),
        "remove `types`; this event has no activity types",
    )
}

#[derive(Clone, Copy)]
enum FilterKind {
    Branch,
    Tag,
    Path,
}

impl FilterKind {
    fn supported(self, event: &str) -> bool {
        match self {
            Self::Branch => catalog::supports_branch_filter(event),
            Self::Tag => catalog::supports_tag_filter(event),
            Self::Path => catalog::supports_path_filter(event),
        }
    }

    fn help(self) -> String {
        let (filters, events) = match self {
            Self::Branch => ("branches/branches-ignore", catalog::BRANCH_FILTER_EVENTS),
            Self::Tag => ("tags/tags-ignore", catalog::TAG_FILTER_EVENTS),
            Self::Path => ("paths/paths-ignore", catalog::PATH_FILTER_EVENTS),
        };
        format!("`{filters}` is only available for {}", list(events))
    }
}

fn check_filters(
    event: &str,
    config: &Map<String, Value>,
    path: &str,
    findings: &mut Vec<Diagnostic>,
) {
    for (key, kind) in [
        ("branches", FilterKind::Branch),
        ("branches-ignore", FilterKind::Branch),
        ("tags", FilterKind::Tag),
        ("tags-ignore", FilterKind::Tag),
        ("paths", FilterKind::Path),
        ("paths-ignore", FilterKind::Path),
    ] {
        let Some(value) = config.get(key) else {
            continue;
        };
        if !filter_is_set(value) {
            continue;
        }
        let filter_path = format!("{path}/{}", escape(key));
        if !kind.supported(event) {
            findings.push(error(
                "preloop/invalid-event-filter",
                &filter_path,
                format!("`{key}` is not available for the `{event}` event"),
                &kind.help(),
            ));
            continue;
        }
        check_globs(kind, value, &filter_path, findings);
    }

    for (include, ignore, kind) in [
        ("branches", "branches-ignore", FilterKind::Branch),
        ("tags", "tags-ignore", FilterKind::Tag),
        ("paths", "paths-ignore", FilterKind::Path),
    ] {
        if kind.supported(event)
            && config.get(include).is_some_and(filter_is_set)
            && config.get(ignore).is_some_and(filter_is_set)
        {
            findings.push(error(
                "preloop/invalid-event-filter",
                &format!("{path}/{}", escape(ignore)),
                format!("`{include}` and `{ignore}` cannot both be set for the `{event}` event"),
                "use one filter and `!` negation patterns inside it",
            ));
        }
    }
}

fn filter_is_set(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => !text.is_empty(),
        Value::Array(values) => !values.is_empty(),
        _ => true,
    }
}

fn check_globs(kind: FilterKind, value: &Value, path: &str, findings: &mut Vec<Diagnostic>) {
    match value {
        Value::String(pattern) => check_glob(kind, pattern, path, findings),
        Value::Array(patterns) => {
            for (index, pattern) in patterns.iter().enumerate() {
                if let Value::String(pattern) = pattern {
                    check_glob(kind, pattern, &format!("{path}/{index}"), findings);
                }
            }
        }
        _ => {}
    }
}

fn check_glob(kind: FilterKind, pattern: &str, path: &str, findings: &mut Vec<Diagnostic>) {
    if pattern.contains("${{") {
        return;
    }
    let invalid = match kind {
        FilterKind::Branch | FilterKind::Tag => glob::validate_ref_glob(pattern),
        FilterKind::Path => glob::validate_path_glob(pattern),
    };
    if let Some(reason) = invalid {
        findings.push(error(
            "preloop/invalid-glob",
            path,
            format!("filter pattern `{pattern}` is invalid: {reason}"),
            "see https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#filter-pattern-cheat-sheet",
        ));
    }
}

pub(super) fn error(
    rule_id: &str,
    path: &str,
    message: impl Into<String>,
    help: &str,
) -> Diagnostic {
    let mut finding = crate::diagnostic(rule_id, path, message.into(), help);
    finding.severity = Severity::Error;
    finding.confidence = Confidence::High;
    finding
}

pub(super) fn warning(
    rule_id: &str,
    path: &str,
    message: impl Into<String>,
    help: &str,
) -> Diagnostic {
    crate::diagnostic(rule_id, path, message.into(), help)
}

pub(super) fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn list(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use preloop_gha_authoring::WorkflowDocument;

    use crate::{Diagnostic, LintContext, Severity};

    use super::check;

    fn check_yaml(yaml: &str) -> Vec<Diagnostic> {
        let document = WorkflowDocument::parse(yaml).expect("parse workflow");
        let mut findings = Vec::new();
        check(document.value(), &mut findings);
        findings
    }

    fn rules(yaml: &str) -> Vec<String> {
        check_yaml(yaml)
            .into_iter()
            .map(|finding| finding.rule_id)
            .collect()
    }

    #[test]
    fn accepts_string_list_and_mapping_forms() {
        for yaml in [
            "on: push\n",
            "on: workflow_dispatch\n",
            "on: [push, pull_request]\n",
            "on:\n  push:\n  pull_request:\n",
            "on:\n  push: {}\n  workflow_dispatch: {}\n",
        ] {
            assert!(
                check_yaml(yaml).is_empty(),
                "{yaml}: {:?}",
                check_yaml(yaml)
            );
        }
    }

    #[test]
    fn rejects_unknown_event_in_every_form() {
        for yaml in ["on: pushh\n", "on: [push, pushh]\n", "on:\n  pushh:\n"] {
            let findings = check_yaml(yaml);
            assert_eq!(findings.len(), 1, "{yaml}: {findings:?}");
            assert_eq!(findings[0].rule_id, "preloop/unknown-event");
            assert_eq!(findings[0].severity, Severity::Error);
        }
    }

    #[test]
    fn expression_event_names_are_unknown() {
        assert!(check_yaml("on: \"${{ inputs.event }}\"\n").is_empty());
        assert!(check_yaml("on:\n  \"${{ inputs.event }}\":\n").is_empty());
    }

    #[test]
    fn rejects_unknown_activity_type() {
        let findings = check_yaml("on:\n  pull_request:\n    types: [opend]\n");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule_id, "preloop/unknown-activity-type");
        assert_eq!(findings[0].path, "/on/pull_request/types/0");
    }

    #[test]
    fn accepts_valid_activity_types() {
        assert!(check_yaml("on:\n  pull_request:\n    types: [opened, synchronize]\n").is_empty());
        assert!(check_yaml("on:\n  release:\n    types: published\n").is_empty());
    }

    #[test]
    fn rejects_types_on_events_without_activity_types() {
        let findings = check_yaml("on:\n  push:\n    types: [opened]\n");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule_id, "preloop/unknown-activity-type");
    }

    #[test]
    fn expression_activity_types_are_unknown() {
        assert!(
            check_yaml("on:\n  pull_request:\n    types: [\"${{ inputs.type }}\"]\n").is_empty()
        );
    }

    #[test]
    fn rejects_filters_on_events_that_do_not_support_them() {
        let findings = check_yaml("on:\n  issues:\n    branches: [main]\n");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule_id, "preloop/invalid-event-filter");
        assert_eq!(findings[0].path, "/on/issues/branches");
    }

    #[test]
    fn accepts_filters_on_supporting_events() {
        assert!(check_yaml("on:\n  push:\n    branches: [main]\n    tags: [v*]\n").is_empty());
        assert!(
            check_yaml("on:\n  workflow_run:\n    branches: [main]\n    types: [completed]\n")
                .is_empty()
        );
        assert!(check_yaml("on:\n  pull_request:\n    paths-ignore: ['**.md']\n").is_empty());
    }

    #[test]
    fn rejects_include_and_ignore_filters_together() {
        let findings =
            check_yaml("on:\n  push:\n    branches: [main]\n    branches-ignore: [dev]\n");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule_id, "preloop/invalid-event-filter");
    }

    #[test]
    fn expression_filter_values_are_unknown() {
        assert!(check_yaml("on:\n  push:\n    branches: [\"${{ inputs.branch }}\"]\n").is_empty());
    }

    #[test]
    fn rejects_malformed_globs() {
        let findings = check_yaml("on:\n  push:\n    branches: ['[abc']\n");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule_id, "preloop/invalid-glob");
        assert_eq!(findings[0].path, "/on/push/branches/0");

        let findings = check_yaml("on:\n  pull_request:\n    paths: ['../src']\n");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule_id, "preloop/invalid-glob");
    }

    #[test]
    fn accepts_valid_globs() {
        assert!(
            check_yaml("on:\n  push:\n    branches: [main, 'releases/**']\n    tags: ['**[0-9]+.[0-9]+*']\n")
                .is_empty()
        );
        assert!(
            check_yaml("on:\n  pull_request:\n    paths: ['crates/**', '!docs/**']\n").is_empty()
        );
    }

    #[test]
    fn expression_globs_are_unknown() {
        assert!(check_yaml("on:\n  push:\n    branches: ['${{ matrix.branch }}']\n").is_empty());
    }

    #[test]
    fn rejects_invalid_cron() {
        let findings = check_yaml("on:\n  schedule:\n    - cron: '0 0 * *'\n");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule_id, "preloop/invalid-cron");
        assert_eq!(findings[0].path, "/on/schedule/0/cron");
    }

    #[test]
    fn accepts_valid_cron() {
        assert!(
            check_yaml("on:\n  schedule:\n    - cron: '0 4 * * 1'\n    - cron: '*/15 * * * *'\n")
                .is_empty()
        );
    }

    #[test]
    fn warns_about_cron_below_five_minutes() {
        let findings = check_yaml("on:\n  schedule:\n    - cron: '* * * * *'\n");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule_id, "preloop/cron-too-frequent");
        assert_eq!(findings[0].severity, Severity::Warning);
    }

    #[test]
    fn does_not_warn_about_five_minute_cron() {
        assert!(check_yaml("on:\n  schedule:\n    - cron: '*/5 * * * *'\n").is_empty());
    }

    #[test]
    fn expression_cron_is_unknown() {
        assert!(check_yaml("on:\n  schedule:\n    - cron: \"${{ inputs.cron }}\"\n").is_empty());
    }

    #[test]
    fn validates_timezone_shape() {
        assert!(
            check_yaml(
                "on:\n  schedule:\n    - cron: '0 4 * * 1'\n      timezone: 'America/New_York'\n"
            )
            .is_empty()
        );
        let findings =
            check_yaml("on:\n  schedule:\n    - cron: '0 4 * * 1'\n      timezone: 'Local'\n");
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule_id, "preloop/invalid-cron");
        assert_eq!(findings[0].path, "/on/schedule/0/timezone");
    }

    #[test]
    fn rejects_invalid_dispatch_inputs() {
        let findings = check_yaml(
            "on:\n  workflow_dispatch:\n    inputs:\n      profile:\n        type: select\n",
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule_id, "preloop/invalid-dispatch-input");
        assert_eq!(
            findings[0].path,
            "/on/workflow_dispatch/inputs/profile/type"
        );
    }

    #[test]
    fn accepts_valid_dispatch_inputs() {
        assert!(
            check_yaml(
                "on:\n  workflow_dispatch:\n    inputs:\n      profile:\n        type: choice\n        options: [fast, intensive]\n        default: fast\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn rejects_workflow_call_outputs_without_value() {
        let findings = check_yaml(
            "on:\n  workflow_call:\n    outputs:\n      result:\n        description: missing\n",
        );
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].rule_id, "preloop/invalid-dispatch-input");
        assert_eq!(findings[0].path, "/on/workflow_call/outputs/result/value");
    }

    #[test]
    fn lint_workflow_registers_trigger_checks() {
        let document = WorkflowDocument::parse("on: pushh\n").expect("parse workflow");
        let findings = crate::lint_workflow(&document, &LintContext::default());
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule_id == "preloop/unknown-event"),
            "{findings:?}"
        );
    }
}

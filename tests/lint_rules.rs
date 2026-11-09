//! Behavioral tests for the schema, runner, script and condition rules in
//! `preloop-gha-lint`, through the same `lint_workflow` entry point that
//! `loopt lint` uses.

use std::collections::BTreeSet;

use preloop_gha_authoring::WorkflowDocument;
use preloop_gha_lint::{lint_workflow, Diagnostic, LintContext, Severity};
use serde_json::{json, Value};

fn lint(value: Value) -> Vec<Diagnostic> {
    lint_with(value, &LintContext::default())
}

fn lint_with(value: Value, context: &LintContext) -> Vec<Diagnostic> {
    let document = WorkflowDocument::from_value(value).expect("workflow document");
    lint_workflow(&document, context)
}

fn with_rule<'a>(findings: &'a [Diagnostic], rule: &str) -> Vec<&'a Diagnostic> {
    findings
        .iter()
        .filter(|finding| finding.rule_id == rule)
        .collect()
}

fn job(runs_on: Value, steps: Value) -> Value {
    json!({
        "runs-on": runs_on,
        "steps": steps,
    })
}

fn workflow(jobs: Value) -> Value {
    json!({
        "name": "ci",
        "on": "push",
        "jobs": jobs,
    })
}

#[test]
fn unknown_keys_are_warnings_with_a_suggestion() {
    let findings = lint(workflow(json!({
        "build": {
            "runs_on": "ubuntu-24.04",
            "steps": [
                {"run": "echo hi", "timeout_minutes": 5},
                {"run": "echo hi", "Shell": "bash"},
            ],
        },
    })));

    let unknown = with_rule(&findings, "preloop/unknown-key");
    assert_eq!(unknown.len(), 3, "{unknown:#?}");
    assert!(unknown
        .iter()
        .all(|finding| finding.severity == Severity::Warning));
    assert!(unknown.iter().any(|finding| {
        finding.path == "/jobs/build/runs_on"
            && finding
                .help
                .as_deref()
                .is_some_and(|help| help.contains("runs-on"))
    }));
    assert!(unknown.iter().any(|finding| {
        finding.path == "/jobs/build/steps/0/timeout_minutes"
            && finding
                .help
                .as_deref()
                .is_some_and(|help| help.contains("timeout-minutes"))
    }));
    assert!(unknown.iter().any(|finding| {
        finding.path == "/jobs/build/steps/1/Shell"
            && finding
                .help
                .as_deref()
                .is_some_and(|help| help.contains("shell"))
    }));
}

#[test]
fn known_keys_at_every_level_are_not_reported() {
    let findings = lint(json!({
        "name": "ci",
        "run-name": "run",
        "on": "push",
        "permissions": {"contents": "read"},
        "env": {"FOO": "bar"},
        "defaults": {"run": {"shell": "bash", "working-directory": "."}},
        "concurrency": {"group": "ci", "cancel-in-progress": true, "queue": "max"},
        "jobs": {
            "build": {
                "name": "build",
                "runs-on": "ubuntu-24.04",
                "timeout-minutes": 10,
                "outputs": {"out": "value"},
                "env": {"B": "c"},
                "strategy": {
                    "fail-fast": false,
                    "max-parallel": 2,
                    "matrix": {"os": ["ubuntu-24.04"]},
                },
                "container": {
                    "image": "node:20",
                    "credentials": {"username": "u", "password": "p"},
                    "env": {"A": "b"},
                    "ports": [80],
                    "volumes": ["a:b"],
                    "options": "--cpus 1",
                },
                "services": {
                    "redis": {
                        "image": "redis",
                        "credentials": {"username": "u", "password": "p"},
                        "env": {"A": "b"},
                        "ports": [6379],
                        "volumes": ["a:b"],
                        "options": "--cpus 1",
                        "command": "redis-server",
                        "entrypoint": "docker-entrypoint.sh",
                    },
                },
                "environment": {
                    "name": "production",
                    "url": "https://example.com",
                    "deployment": false,
                },
                "concurrency": {"group": "g", "cancel-in-progress": false, "queue": "single"},
                "snapshot": {
                    "if": "${{ github.ref == 'refs/heads/main' }}",
                    "image-name": "image",
                    "version": "1.*",
                },
                "defaults": {"run": {"shell": "bash", "working-directory": "."}},
                "steps": [
                    {
                        "name": "script",
                        "id": "script",
                        "if": "${{ github.event_name == 'push' }}",
                        "run": "echo hi",
                        "shell": "bash",
                        "working-directory": ".",
                        "env": {"C": "d"},
                        "continue-on-error": true,
                        "timeout-minutes": 1,
                    },
                    {"uses": "actions/checkout@v4", "with": {"fetch-depth": 0}},
                    {"parallel": [{"run": "echo a"}, {"run": "echo b"}]},
                    {"wait": ["script"]},
                    {"wait-all": null},
                    {"cancel": "script"},
                ],
            },
            "call": {
                "uses": "owner/repo/.github/workflows/ci.yml@v1",
                "with": {"input": "value"},
                "secrets": {"TOKEN": "${{ secrets.TOKEN }}"},
                "strategy": {"matrix": {"target": ["a"]}},
                "needs": ["build"],
                "if": "always()",
                "permissions": {"contents": "read"},
                "concurrency": {"group": "g"},
                "cache-mode": "read",
            },
        },
    }));

    let unknown = with_rule(&findings, "preloop/unknown-key");
    assert!(unknown.is_empty(), "{unknown:#?}");
}

#[test]
fn invalid_job_and_step_ids_are_errors() {
    let findings = lint(workflow(json!({
        "2d-game": job(
            json!("ubuntu-24.04"),
            json!([{"id": "echo for test", "run": "echo hi"}]),
        ),
        "_ok": job(
            json!("ubuntu-24.04"),
            json!([{"id": "build-2", "run": "echo hi"}]),
        ),
    })));

    let ids = with_rule(&findings, "preloop/invalid-id");
    assert_eq!(ids.len(), 2, "{ids:#?}");
    assert!(ids
        .iter()
        .all(|finding| finding.severity == Severity::Error));
    assert!(ids
        .iter()
        .any(|finding| finding.path == "/jobs/2d-game" && finding.message.contains("job")));
    assert!(ids
        .iter()
        .any(|finding| finding.path == "/jobs/2d-game/steps/0/id"
            && finding.message.contains("step")));
}

#[test]
fn invalid_environment_variable_names_are_reported() {
    let findings = lint(workflow(json!({
        "build": {
            "runs-on": "ubuntu-24.04",
            "env": {
                "FOO=BAR": "value",
                "1FOO": "value",
                "GITHUB_SHA": "value",
                "GITHUB_TOKEN": "${{ secrets.GITHUB_TOKEN }}",
            },
            "steps": [{"run": "echo hi"}],
        },
    })));

    let names = with_rule(&findings, "preloop/invalid-env-name");
    assert_eq!(names.len(), 3, "{names:#?}");
    assert!(names
        .iter()
        .any(|finding| finding.path == "/jobs/build/env/FOO=BAR"
            && finding.severity == Severity::Error));
    assert!(names
        .iter()
        .any(|finding| finding.path == "/jobs/build/env/1FOO"
            && finding.severity == Severity::Warning));
    assert!(names
        .iter()
        .any(|finding| finding.path == "/jobs/build/env/GITHUB_SHA"
            && finding.severity == Severity::Warning));
    assert!(!names
        .iter()
        .any(|finding| finding.path.contains("GITHUB_TOKEN")));
}

#[test]
fn invalid_permission_scopes_and_values_are_errors() {
    let findings = lint(json!({
        "on": "push",
        "permissions": "write",
        "jobs": {
            "build": {
                "runs-on": "ubuntu-24.04",
                "permissions": {
                    "check": "write",
                    "issues": "readable",
                    "models": "write",
                    "id-token": "read",
                },
                "steps": [{"run": "echo hi"}],
            },
        },
    }));

    let permissions = with_rule(&findings, "preloop/invalid-permission");
    assert_eq!(permissions.len(), 5, "{permissions:#?}");
    assert!(permissions
        .iter()
        .all(|finding| finding.severity == Severity::Error));
    assert!(permissions
        .iter()
        .any(|finding| finding.path == "/permissions"));
    assert!(permissions
        .iter()
        .any(|finding| finding.path == "/jobs/build/permissions/check"));
    assert!(permissions
        .iter()
        .any(|finding| finding.path == "/jobs/build/permissions/models"));
    assert!(permissions
        .iter()
        .any(|finding| finding.path == "/jobs/build/permissions/id-token"));
}

#[test]
fn valid_and_expression_permissions_are_accepted() {
    let findings = lint(json!({
        "on": "push",
        "permissions": "read-all",
        "jobs": {
            "build": {
                "runs-on": "ubuntu-24.04",
                "permissions": {
                    "contents": "read",
                    "id-token": "write",
                    "models": "read",
                    "attestations": "write",
                    "copilot-requests": "write",
                    "vulnerability-alerts": "read",
                    "artifact-metadata": "write",
                    "code-quality": "write",
                },
                "steps": [{"run": "echo hi"}],
            },
            "call": {
                "uses": "owner/repo/.github/workflows/ci.yml@v1",
                "permissions": "${{ fromJSON(inputs.permissions) }}",
            },
        },
    }));

    let permissions = with_rule(&findings, "preloop/invalid-permission");
    assert!(permissions.is_empty(), "{permissions:#?}");
}

#[test]
fn unknown_runner_labels_are_warnings() {
    let findings = lint(workflow(json!({
        "build": job(
            json!(["linux-latest", "my-custom-runner"]),
            json!([{"run": "echo hi"}]),
        ),
    })));

    let labels = with_rule(&findings, "preloop/unknown-runner-label");
    assert_eq!(labels.len(), 2, "{labels:#?}");
    assert!(labels
        .iter()
        .all(|finding| finding.severity == Severity::Warning));
    assert!(labels.iter().all(|finding| {
        finding
            .help
            .as_deref()
            .is_some_and(|help| help.contains("--context"))
    }));
    assert!(labels
        .iter()
        .any(|finding| finding.path == "/jobs/build/runs-on/0"));
}

#[test]
fn known_runner_labels_and_expression_selectors_are_accepted() {
    let findings = lint(workflow(json!({
        "a": job(json!("ubuntu-22.04"), json!([{"run": "echo hi"}])),
        "b": job(json!("windows-2025"), json!([{"run": "echo hi"}])),
        "c": job(json!("macos-15-xlarge"), json!([{"run": "echo hi"}])),
        "d": job(json!("ubuntu-24.04-8-core"), json!([{"run": "echo hi"}])),
        "e": job(json!("arm64"), json!([{"run": "echo hi"}])),
        "f": job(
            json!(["self-hosted", "preloop-cpane"]),
            json!([{"run": "echo hi"}]),
        ),
        "g": job(json!("${{ matrix.os }}"), json!([{"run": "echo hi"}])),
        "h": job(json!({"group": "runners"}), json!([{"run": "echo hi"}])),
        "i": {
            "runs-on": "not-a-real-label",
            "if": false,
            "steps": [{"run": "echo hi"}],
        },
    })));

    let labels = with_rule(&findings, "preloop/unknown-runner-label");
    assert!(labels.is_empty(), "{labels:#?}");
}

#[test]
fn supplied_runner_inventory_supersedes_unknown_label_warnings() {
    let context = LintContext {
        runner_label_sets: Some(vec![BTreeSet::from(["my-custom-runner".to_owned()])]),
        ..Default::default()
    };
    let findings = lint_with(
        workflow(json!({
            "build": job(json!("my-custom-runner"), json!([{"run": "echo hi"}])),
        })),
        &context,
    );

    let labels = with_rule(&findings, "preloop/unknown-runner-label");
    assert!(labels.is_empty(), "{labels:#?}");
}

#[test]
fn latest_runner_images_are_informational() {
    let findings = lint(workflow(json!({
        "a": job(json!("ubuntu-latest"), json!([{"run": "echo hi"}])),
        "b": job(json!("macos-latest"), json!([{"run": "echo hi"}])),
        "c": job(json!("ubuntu-22.04"), json!([{"run": "echo hi"}])),
    })));

    let floating = with_rule(&findings, "preloop/floating-runner-image");
    assert_eq!(floating.len(), 2, "{floating:#?}");
    assert!(floating
        .iter()
        .all(|finding| finding.severity == Severity::Info));
    assert!(floating
        .iter()
        .any(|finding| finding.path == "/jobs/a/runs-on"));
}

#[test]
fn invalid_shell_names_are_errors() {
    let findings = lint(workflow(json!({
        "build": job(
            json!("ubuntu-24.04"),
            json!([
                {"run": "echo hi", "shell": "fish"},
                {"run": "echo hi", "shell": "python3"},
                {"run": "echo hi", "shell": "bash"},
                {"run": "echo hi", "shell": "perl {0}"},
                {"run": "echo hi", "shell": "${{ matrix.shell }}"},
                {"uses": "actions/checkout@v4", "shell": "fish"},
            ]),
        ),
    })));

    let shells = with_rule(&findings, "preloop/invalid-shell");
    assert_eq!(shells.len(), 2, "{shells:#?}");
    assert!(shells
        .iter()
        .all(|finding| finding.severity == Severity::Error));
}

#[test]
fn deprecated_workflow_commands_are_warnings() {
    let findings = lint(workflow(json!({
        "build": job(
            json!("ubuntu-24.04"),
            json!([
                {"run": "echo '::set-output name=foo::bar'"},
                {"run": "echo '::save-state name=state::value'"},
                {"run": "echo '::set-env name=FOO::bar'"},
                {"run": "echo '::add-path::/opt/bin'"},
                {"run": "echo \"foo=bar\" >> \"$GITHUB_OUTPUT\""},
                {"run": "echo '::debug::Set the variable'"},
                {"run": "echo 'do not use ::set-output in scripts'"},
            ]),
        ),
    })));

    let commands = with_rule(&findings, "preloop/deprecated-command");
    assert_eq!(commands.len(), 4, "{commands:#?}");
    assert!(commands
        .iter()
        .all(|finding| finding.severity == Severity::Warning));
    assert!(commands.iter().any(|finding| {
        finding.message.contains("set-output")
            && finding
                .help
                .as_deref()
                .is_some_and(|help| help.contains("$GITHUB_OUTPUT"))
    }));
    assert!(commands.iter().any(|finding| {
        finding.message.contains("add-path")
            && finding
                .help
                .as_deref()
                .is_some_and(|help| help.contains("$GITHUB_PATH"))
    }));
}

#[test]
fn constant_conditions_are_warnings() {
    let findings = lint(workflow(json!({
        "bool": {
            "runs-on": "ubuntu-24.04",
            "if": false,
            "steps": [{"run": "echo hi"}],
        },
        "literal": {
            "runs-on": "ubuntu-24.04",
            "steps": [
                {"run": "echo hi", "if": "false"},
                {"run": "echo hi", "if": "${{ 1 == 2 }}"},
                {"run": "echo hi", "if": "${{ true }}"},
                {"run": "echo hi", "if": "${{ github.event_name == 'push' }} && ${{ github.ref == 'refs/heads/main' }}"},
                {"run": "echo hi", "if": "${{ github.event_name == 'push' }} "},
            ],
        },
    })));

    let conditions = with_rule(&findings, "preloop/constant-condition");
    assert_eq!(conditions.len(), 6, "{conditions:#?}");
    assert!(conditions
        .iter()
        .all(|finding| finding.severity == Severity::Warning));
    assert!(conditions
        .iter()
        .any(|finding| finding.path == "/jobs/bool/if"));
    assert!(conditions
        .iter()
        .any(|finding| finding.path == "/jobs/literal/steps/3/if"
            && finding.message.contains("surrounds")));
}

#[test]
fn dynamic_conditions_are_accepted() {
    let findings = lint(workflow(json!({
        "build": {
            "runs-on": "ubuntu-24.04",
            "steps": [
                {"run": "echo hi", "if": "${{ github.event_name == 'push' }}"},
                {"run": "echo hi", "if": "github.event_name == 'push'"},
                {"run": "echo hi", "if": "${{ success() }}"},
                {"run": "echo hi", "if": "${{ hashFiles('**/Cargo.lock') != '' }}"},
                {"run": "echo hi", "if": "${{ matrix.target }}"},
                {"run": "echo hi", "if": "${{ env.FLAG }}"},
            ],
        },
    })));

    let conditions = with_rule(&findings, "preloop/constant-condition");
    assert!(conditions.is_empty(), "{conditions:#?}");
}

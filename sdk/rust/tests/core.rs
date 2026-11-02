//! Builder/JSON-output tests for `loopt-sdk`'s `core` module: step, job,
//! and workflow construction, field order, and `JobRef`/`StepRef`
//! references.

use loopt_sdk::core::{call_workflow, job, run, uses, workflow};
use loopt_sdk::expr::{eq, github};
use loopt_sdk::Error;
use serde_json::json;

#[test]
fn run_step_builds_ordered_fields() {
    let step = run("echo hi")
        .id("greet")
        .name("Greet")
        .with("x", "y") // ignored by a run step's own semantics, exercises the builder
        .env("GREETING", "hi")
        .shell("bash")
        .working_directory("sub")
        .continue_on_error(true)
        .timeout_minutes(5)
        .build();

    let value = step.value();
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        vec![
            "id",
            "name",
            "run",
            "with",
            "shell",
            "working-directory",
            "env",
            "continue-on-error",
            "timeout-minutes",
        ]
    );
    assert_eq!(value["run"], json!("echo hi"));
    assert_eq!(value["env"], json!({"GREETING": "hi"}));
}

#[test]
fn uses_step_without_optional_fields_omits_them() {
    let step = uses("actions/checkout@v6").build();
    let value = step.value();
    assert_eq!(value, &json!({"uses": "actions/checkout@v6"}));
}

#[test]
fn if_condition_renders_wrapped_expression() {
    let step = run("echo hi")
        .if_cond(eq(github().field("ref"), "refs/heads/main").unwrap())
        .build();
    assert_eq!(
        step.value()["if"],
        json!("${{ github.ref == 'refs/heads/main' }}")
    );
}

#[test]
fn step_ref_output_requires_an_id() {
    let step = run("echo hi").build();
    assert_eq!(step.output("x").unwrap_err(), Error::MissingStepId);

    let step = run("echo hi").id("s").build();
    assert_eq!(
        step.output("x").unwrap().to_string(),
        "${{ steps.s.outputs.x }}"
    );
    assert_eq!(
        step.outcome().unwrap().to_string(),
        "${{ steps.s.outcome }}"
    );
    assert_eq!(
        step.conclusion().unwrap().to_string(),
        "${{ steps.s.conclusion }}"
    );
}

#[test]
fn job_builds_ordered_fields_and_declares_outputs() {
    let build = job()
        .name("Build")
        .runs_on("ubuntu-latest")
        .timeout_minutes(10)
        .concurrency("build-${{ github.ref }}")
        .env("CI", "true")
        .output("sha", "${{ steps.checkout.outputs.sha }}")
        .step(uses("actions/checkout@v6").id("checkout").build())
        .build();

    let keys: Vec<&String> = build.value().as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        vec![
            "name",
            "runs-on",
            "timeout-minutes",
            "concurrency",
            "env",
            "outputs",
            "steps"
        ]
    );
    assert_eq!(
        build.value()["steps"].as_array().unwrap().len(),
        1,
        "step must land in the steps array"
    );
}

#[test]
fn job_ref_output_validates_against_declared_outputs() {
    let (wf, build_ref) = workflow().job(
        "build",
        job()
            .runs_on("ubuntu-latest")
            .output("sha", "abc")
            .step(run("echo hi").build())
            .build(),
    );
    let _ = wf;

    assert_eq!(build_ref.id(), "build");
    assert_eq!(
        build_ref.output("sha").unwrap().to_string(),
        "${{ needs.build.outputs.sha }}"
    );
    let err = build_ref.output("missing").unwrap_err();
    assert!(matches!(err, Error::UndeclaredOutput { kind: "job", .. }));
    assert_eq!(build_ref.result().to_string(), "${{ needs.build.result }}");
}

#[test]
fn needs_lowers_to_string_for_one_dependency_and_array_for_many() {
    let (wf, build_ref) = workflow().job("build", job().runs_on("ubuntu-latest").build());
    let (wf, test_ref) = wf.job(
        "test",
        job().runs_on("ubuntu-latest").needs(&[&build_ref]).build(),
    );
    let (wf, _deploy_ref) = wf.job(
        "deploy",
        job()
            .runs_on("ubuntu-latest")
            .needs(&[&build_ref, &test_ref])
            .build(),
    );
    let document = wf.build();

    assert_eq!(document["jobs"]["test"]["needs"], json!("build"));
    assert_eq!(
        document["jobs"]["deploy"]["needs"],
        json!(["build", "test"])
    );
}

#[test]
fn call_workflow_job_declares_no_restricted_outputs() {
    let (wf, call_ref) = workflow().job(
        "call",
        call_workflow("./.github/workflows/reusable.yml")
            .with("flag", true)
            .secrets("token", "${{ secrets.TOKEN }}")
            .concurrency("call-${{ github.ref }}")
            .build(),
    );
    let document = wf.build();

    assert_eq!(
        document["jobs"]["call"],
        json!({
            "uses": "./.github/workflows/reusable.yml",
            "with": {"flag": true},
            "secrets": {"token": "${{ secrets.TOKEN }}"},
            "concurrency": "call-${{ github.ref }}",
        })
    );
    // Reusable-workflow calls never restrict output names.
    assert!(call_ref.output("anything-at-all").is_ok());
}

#[test]
fn workflow_builds_fields_in_github_conventional_order() {
    let (wf, _build_ref) = workflow()
        .name("CI")
        .on("push")
        .permissions(json!({"contents": "read"}))
        .env("CI", "true")
        .concurrency(json!({"group": "ci-${{ github.ref }}"}))
        .defaults(json!({"run": {"shell": "bash"}}))
        .job("build", job().runs_on("ubuntu-latest").build());
    let document = wf.build();

    let keys: Vec<&String> = document.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        vec![
            "name",
            "on",
            "permissions",
            "env",
            "concurrency",
            "defaults",
            "jobs"
        ]
    );
    assert_eq!(document["jobs"]["build"]["runs-on"], json!("ubuntu-latest"));
}

#[test]
fn workflow_with_no_optional_fields_still_has_jobs() {
    let document = workflow().build();
    assert_eq!(document, json!({"jobs": {}}));
}

//! Behavioral tests for `loopt lint` and the lint review attached to
//! `loopt gen`. Each test runs the real CLI; nothing is mocked.

mod support;

use support::TestRepo;

fn workflow(permissions: &str, jobs: &str) -> String {
    format!("name: ci\non: push\n{permissions}\njobs:\n{jobs}")
}

fn job(name: &str) -> String {
    format!("  {name}:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo {name}\n")
}

#[test]
fn lint_reports_security_errors_and_exits_nonzero() {
    let repo = TestRepo::new("lint-error");
    repo.write(
        ".github/workflows/ci.yml",
        &workflow("permissions: write-all", &(job("build") + &job("test"))),
    );

    let output = repo.run(&["lint", "--json"]);
    output.assert_failure();
    let reports: serde_json::Value =
        serde_json::from_str(&output.stdout).expect("lint JSON is valid");
    let rule_ids: Vec<&str> = reports
        .as_array()
        .expect("reports array")
        .iter()
        .flat_map(|report| report["findings"].as_array().expect("findings array"))
        .filter_map(|finding| finding["rule_id"].as_str())
        .collect();
    assert!(
        rule_ids.contains(&"excessive-permissions"),
        "expected excessive-permissions in {rule_ids:?}"
    );
}

#[test]
fn lint_warnings_exit_zero_and_pedantic_reveals_policy_findings() {
    let repo = TestRepo::new("lint-warning");
    repo.write(".github/workflows/ci.yml", &workflow("", &job("build")));

    repo.run(&["lint"]).assert_success();
    let output = repo.run(&["lint", "--persona", "pedantic", "--json"]);
    output.assert_success();
    let reports: serde_json::Value =
        serde_json::from_str(&output.stdout).expect("lint JSON is valid");
    let rule_ids: Vec<&str> = reports
        .as_array()
        .expect("reports array")
        .iter()
        .flat_map(|report| report["findings"].as_array().expect("findings array"))
        .filter_map(|finding| finding["rule_id"].as_str())
        .collect();
    assert!(
        rule_ids.contains(&"anonymous-definition"),
        "expected anonymous-definition in {rule_ids:?}"
    );
}

#[test]
fn lint_accepts_an_explicit_workflow_path() {
    let repo = TestRepo::new("lint-explicit");
    repo.write(
        "ci.yml",
        &workflow("permissions: {contents: read}", &job("build")),
    );
    let output = repo.run(&["lint", "ci.yml", "--json"]);
    output.assert_success();
    let reports: serde_json::Value =
        serde_json::from_str(&output.stdout).expect("lint JSON is valid");
    assert_eq!(reports.as_array().expect("reports array").len(), 1);
}

#[test]
fn gen_runs_the_review_without_failing_generation() {
    let repo = TestRepo::new("gen-lint");
    repo.write(
        ".loopt/workflows/ci.ts",
        r#"
const workflow = {
  name: "ci",
  on: "push",
  permissions: "write-all",
  jobs: {
    build: { "runs-on": "ubuntu-latest", steps: [{ run: "echo build" }] },
    test: { "runs-on": "ubuntu-latest", steps: [{ run: "echo test" }] },
  },
};
console.log(JSON.stringify(workflow));
"#,
    );

    let output = repo.run(&["gen"]);
    output.assert_success();
    assert!(repo.exists(".github/workflows/ci.yml"));
    assert!(
        output.stderr.contains("excessive-permissions"),
        "gen should report the review on stderr: {}",
        output.stderr
    );

    let quiet = repo.run(&["gen", "--no-lint"]);
    quiet.assert_success();
    assert!(
        !quiet.stderr.contains("excessive-permissions"),
        "--no-lint should suppress the review: {}",
        quiet.stderr
    );
}

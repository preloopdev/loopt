//! End-to-end tests for `loopt lint`'s external `shellcheck` integration.
//!
//! Every test drives the real `loopt` binary against workflow and action
//! documents written into a fresh temporary repository, exactly like the rest
//! of this suite. Most tests need the real `shellcheck` executable on `PATH`;
//! when it is missing they print why they were skipped, unless
//! `LOOPT_REQUIRE_SHELLCHECK=1` is set, in which case a missing shellcheck is
//! a test failure. The remaining tests are deterministic: they point the CLI
//! at an executable that cannot exist, so they never depend on what the host
//! happens to have installed.

mod support;

use std::process::{Command, Stdio};

use serde_json::Value;
use support::{LooptOutput, TestRepo};

// ---------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------

const UNQUOTED_SCRIPT: &str = r#"name: test
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: echo $FOO
"#;

const QUOTED_SCRIPT: &str = r#"name: test
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: echo "$HOME"
"#;

const MASKED_EXPRESSIONS: &str = r#"name: test
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: echo "${{ github.sha }}"
      - run: echo '${{ matrix.os }}'
      - run: 'if [ "${{ matrix.os }}" = linux ]; then echo hi; fi'
"#;

const EXCLUDED_CODE: &str = r#"name: test
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: echo "$foo"
"#;

const WINDOWS_RUNNERS: &str = r#"name: test
on: push
jobs:
  test:
    runs-on: windows-latest
    steps:
      - run: echo $FOO
      - run: echo $FOO
        shell: bash
  array:
    runs-on: [windows-2022]
    steps:
      - run: echo $FOO
  object:
    runs-on:
      labels: [windows-latest]
    steps:
      - run: echo $FOO
"#;

const NON_POSIX_SHELLS: &str = r#"name: test
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: echo $FOO
        shell: pwsh
      - run: print("hello")
        shell: python
"#;

const JOB_DEFAULT_SH: &str = r#"name: test
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    defaults:
      run:
        shell: sh
    steps:
      - run: '[[ -n x ]]'
      - run: '[[ -n x ]]'
        shell: bash
"#;

const WORKFLOW_DEFAULT_SH: &str = r#"name: test
on: push
defaults:
  run:
    shell: sh
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: '[[ -n x ]]'
"#;

const MULTILINE_SCRIPT: &str = r#"name: test
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: |
          echo hello
          echo $FOO
"#;

const REUSABLE_CALL: &str = r#"name: test
on: push
jobs:
  call:
    uses: example-org/shared-workflows/.github/workflows/ci.yml@0123456789abcdef0123456789abcdef01234567
    with:
      environment: staging
"#;

const SYNTAX_ERROR: &str = r#"name: test
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: if then
"#;

const THIRD_STEP: &str = r#"name: test
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: echo first
      - run: echo second
      - run: echo $THIRD
"#;

const COMPOSITE_ACTION: &str = r#"name: shellcheck composite action
description: Exercises run-script extraction for action documents.
runs:
  using: composite
  steps:
    - run: echo $X
      shell: bash
"#;

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// Whether the real `shellcheck` executable is runnable on this host. Tests
/// that need it return early (after printing a note) when it is absent, so a
/// developer without shellcheck still gets a green suite for everything that
/// does not need the binary. `LOOPT_REQUIRE_SHELLCHECK=1` turns that skip into
/// a failure so CI can demand real coverage.
fn have_shellcheck() -> bool {
    let installed = Command::new("shellcheck")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if installed {
        return true;
    }
    assert!(
        std::env::var("LOOPT_REQUIRE_SHELLCHECK").as_deref() != Ok("1"),
        "LOOPT_REQUIRE_SHELLCHECK=1 but `shellcheck --version` did not succeed; \
         install shellcheck or unset the variable"
    );
    eprintln!("note: `shellcheck` is not installed; skipping this test");
    false
}

/// Writes a workflow at the conventional discovery path and returns the
/// repository-relative path to pass to `loopt lint`.
fn write_workflow(repo: &TestRepo, name: &str, yaml: &str) -> String {
    let relative = format!(".github/workflows/{name}.yml");
    repo.write(relative.as_str(), yaml);
    relative
}

/// One `loopt lint --json <file>` invocation plus the findings it printed.
struct LintRun {
    output: LooptOutput,
    findings: Vec<Value>,
}

impl LintRun {
    /// Findings produced by the external `shellcheck` integration only.
    fn shellcheck_findings(&self) -> Vec<&Value> {
        self.findings
            .iter()
            .filter(|finding| {
                finding
                    .get("rule_id")
                    .and_then(Value::as_str)
                    .is_some_and(|rule_id| rule_id.starts_with("shellcheck/"))
            })
            .collect()
    }
}

/// Runs `loopt lint --json <file> [EXTRA...]` in `repo` and parses the report
/// for that file. Error paths that print no JSON at all read as "no findings".
fn lint(repo: &TestRepo, file: &str, extra: &[&str]) -> LintRun {
    let mut args = vec!["lint", "--json", file];
    args.extend_from_slice(extra);
    let output = repo.run(&args);
    let findings = parse_findings(&output, file);
    LintRun { output, findings }
}

fn parse_findings(output: &LooptOutput, file: &str) -> Vec<Value> {
    if output.stdout.trim().is_empty() {
        return Vec::new();
    }
    let reports: Value = serde_json::from_str(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "`loopt lint --json {file}` printed invalid JSON: {error}\n\
             --- stdout ---\n{}\n--- stderr ---\n{}",
            output.stdout, output.stderr
        )
    });
    let reports = reports
        .as_array()
        .unwrap_or_else(|| panic!("`loopt lint --json` must print an array, got: {reports}"));
    assert_eq!(
        reports.len(),
        1,
        "linting one explicit file must yield exactly one report: {reports:?}"
    );
    reports[0]["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("lint report lacks a findings array: {}", reports[0]))
        .clone()
}

fn text<'a>(finding: &'a Value, field: &str) -> &'a str {
    finding[field]
        .as_str()
        .unwrap_or_else(|| panic!("finding field `{field}` must be a string: {finding}"))
}

// ---------------------------------------------------------------------
// Real shellcheck: what gets reported
// ---------------------------------------------------------------------

#[test]
fn unquoted_variable_is_one_sc2086_finding_with_script_coordinates() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-sc2086");
    let file = write_workflow(&repo, "unquoted", UNQUOTED_SCRIPT);
    let run = lint(&repo, &file, &[]);
    run.output.assert_success();

    let findings = run.shellcheck_findings();
    assert_eq!(
        findings.len(),
        1,
        "expected exactly one shellcheck finding: {findings:?}"
    );
    let finding = findings[0];
    assert_eq!(finding["rule_id"], "shellcheck/SC2086");
    assert_eq!(finding["severity"], "info");
    assert_eq!(finding["confidence"], "high");
    assert_eq!(finding["path"], "/jobs/test/steps/0/run");
    assert_eq!(finding["source"], "shellcheck");
    assert_eq!(finding["help"], "https://www.shellcheck.net/wiki/SC2086");
    assert!(
        finding["line"].is_null(),
        "script coordinates are not workflow coordinates: {finding}"
    );
    assert!(
        finding["column"].is_null(),
        "script coordinates are not workflow coordinates: {finding}"
    );
    let message = text(finding, "message");
    assert!(message.contains("Double quote"), "message: {message}");
    assert!(message.contains("script line 1"), "message: {message}");
    assert!(message.contains("column 6"), "message: {message}");
}

#[test]
fn properly_quoted_script_is_clean() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-quoted");
    let file = write_workflow(&repo, "quoted", QUOTED_SCRIPT);
    let run = lint(&repo, &file, &[]);
    run.output.assert_success();
    assert!(
        run.shellcheck_findings().is_empty(),
        "a quoted expansion must not produce findings: {:?}",
        run.shellcheck_findings()
    );
}

#[test]
fn expressions_are_masked_before_shellcheck_runs() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-expressions");
    let file = write_workflow(&repo, "expressions", MASKED_EXPRESSIONS);
    let run = lint(&repo, &file, &[]);
    run.output.assert_success();
    assert!(
        run.shellcheck_findings().is_empty(),
        "masked expressions must look like plain words to shellcheck: {:?}",
        run.shellcheck_findings()
    );
}

#[test]
fn excluded_sc2154_produces_no_findings() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-excluded");
    let file = write_workflow(&repo, "excluded", EXCLUDED_CODE);
    let run = lint(&repo, &file, &[]);
    run.output.assert_success();
    assert!(
        run.shellcheck_findings().is_empty(),
        "SC2154 is excluded, so an unassigned variable alone is silent: {:?}",
        run.shellcheck_findings()
    );
}

#[test]
fn multiline_script_reports_the_script_relative_line() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-multiline");
    let file = write_workflow(&repo, "multiline", MULTILINE_SCRIPT);
    let run = lint(&repo, &file, &[]);
    run.output.assert_success();

    let findings = run.shellcheck_findings();
    assert_eq!(
        findings.len(),
        1,
        "expected one finding on the unquoted second line: {findings:?}"
    );
    let finding = findings[0];
    assert_eq!(finding["rule_id"], "shellcheck/SC2086");
    assert_eq!(finding["path"], "/jobs/test/steps/0/run");
    let message = text(finding, "message");
    assert!(message.contains("script line 2"), "message: {message}");
}

#[test]
fn third_step_uses_its_own_index_in_the_pointer() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-third-step");
    let file = write_workflow(&repo, "third-step", THIRD_STEP);
    let run = lint(&repo, &file, &[]);
    run.output.assert_success();

    let findings = run.shellcheck_findings();
    assert_eq!(findings.len(), 1, "expected one finding: {findings:?}");
    assert_eq!(findings[0]["rule_id"], "shellcheck/SC2086");
    assert_eq!(findings[0]["path"], "/jobs/test/steps/2/run");
}

#[test]
fn syntax_error_script_is_an_error_finding_and_exits_nonzero() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-syntax-error");
    let file = write_workflow(&repo, "syntax-error", SYNTAX_ERROR);
    let run = lint(&repo, &file, &[]);
    run.output.assert_failure();

    let findings = run.shellcheck_findings();
    assert!(
        !findings.is_empty(),
        "a shell parse failure must be reported: {}",
        run.output.stdout
    );
    let errors = findings
        .iter()
        .filter(|finding| finding.get("severity").and_then(Value::as_str) == Some("error"));
    assert!(
        errors.count() > 0,
        "shellcheck parse failures map to error severity: {findings:?}"
    );
    assert!(
        findings.iter().any(|finding| {
            finding.get("path").and_then(Value::as_str) == Some("/jobs/test/steps/0/run")
        }),
        "findings must point at the offending run step: {findings:?}"
    );
}

#[test]
fn composite_action_scripts_are_linted_at_runs_steps() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-action");
    repo.write("action.yml", COMPOSITE_ACTION);
    let run = lint(&repo, "action.yml", &[]);
    run.output.assert_success();

    let findings = run.shellcheck_findings();
    assert_eq!(
        findings.len(),
        1,
        "expected the composite action's script to be checked: {findings:?}"
    );
    assert_eq!(findings[0]["rule_id"], "shellcheck/SC2086");
    assert_eq!(findings[0]["path"], "/runs/steps/0/run");
}

// ---------------------------------------------------------------------
// Real shellcheck: which scripts are checked at all
// ---------------------------------------------------------------------

#[test]
fn windows_runners_default_to_pwsh_unless_a_posix_shell_is_selected() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-windows");
    let file = write_workflow(&repo, "windows", WINDOWS_RUNNERS);
    let run = lint(&repo, &file, &[]);
    run.output.assert_success();

    let findings = run.shellcheck_findings();
    assert_eq!(
        findings.len(),
        1,
        "only the explicit bash step on a windows runner is checked: {findings:?}"
    );
    assert_eq!(findings[0]["rule_id"], "shellcheck/SC2086");
    assert_eq!(findings[0]["path"], "/jobs/test/steps/1/run");
}

#[test]
fn non_posix_shells_are_skipped() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-other-shells");
    let file = write_workflow(&repo, "other-shells", NON_POSIX_SHELLS);
    let run = lint(&repo, &file, &[]);
    run.output.assert_success();
    assert!(
        run.shellcheck_findings().is_empty(),
        "pwsh and python scripts are not POSIX shell: {:?}",
        run.shellcheck_findings()
    );
}

#[test]
fn job_default_shell_applies_and_a_step_shell_overrides_it() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-job-default");
    let file = write_workflow(&repo, "job-default", JOB_DEFAULT_SH);
    let run = lint(&repo, &file, &[]);
    run.output.assert_success();

    let findings = run.shellcheck_findings();
    assert_eq!(
        findings.len(),
        1,
        "the sh step is checked and the bash override is not: {findings:?}"
    );
    assert_eq!(findings[0]["path"], "/jobs/test/steps/0/run");
    assert!(
        text(findings[0], "rule_id").starts_with("shellcheck/SC"),
        "unexpected rule: {}",
        text(findings[0], "rule_id")
    );
}

#[test]
fn workflow_default_shell_applies_when_the_job_has_none() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-workflow-default");
    let file = write_workflow(&repo, "workflow-default", WORKFLOW_DEFAULT_SH);
    let run = lint(&repo, &file, &[]);
    run.output.assert_success();

    let findings = run.shellcheck_findings();
    assert_eq!(
        findings.len(),
        1,
        "the workflow-level sh default must be honored: {findings:?}"
    );
    assert_eq!(findings[0]["path"], "/jobs/test/steps/0/run");
}

#[test]
fn reusable_call_jobs_are_skipped_without_error() {
    if !have_shellcheck() {
        return;
    }
    let repo = TestRepo::new("shellcheck-reusable-call");
    let file = write_workflow(&repo, "reusable", REUSABLE_CALL);
    let run = lint(&repo, &file, &[]);
    run.output.assert_success();
    assert!(
        run.shellcheck_findings().is_empty(),
        "a job calling a reusable workflow has no run scripts: {:?}",
        run.shellcheck_findings()
    );
}

// ---------------------------------------------------------------------
// Mode handling and missing executables (no real shellcheck required)
// ---------------------------------------------------------------------

#[test]
fn off_mode_disables_the_integration() {
    let repo = TestRepo::new("shellcheck-off");
    let file = write_workflow(&repo, "unquoted", UNQUOTED_SCRIPT);
    let run = lint(&repo, &file, &["--shellcheck", "off"]);
    run.output.assert_success();
    assert!(
        run.shellcheck_findings().is_empty(),
        "`--shellcheck off` must not report: {:?}",
        run.shellcheck_findings()
    );
}

#[test]
fn auto_mode_silently_skips_an_unrunnable_executable() {
    let repo = TestRepo::new("shellcheck-auto-missing");
    let file = write_workflow(&repo, "unquoted", UNQUOTED_SCRIPT);
    let run = lint(
        &repo,
        &file,
        &["--shellcheck-path", "/nonexistent/shellcheck"],
    );
    run.output.assert_success();
    assert!(
        run.shellcheck_findings().is_empty(),
        "auto mode must not invent findings when shellcheck cannot run: {:?}",
        run.shellcheck_findings()
    );
    assert!(
        run.output.stderr.is_empty(),
        "auto mode must skip silently, but stderr said: {}",
        run.output.stderr
    );
}

#[test]
fn on_mode_fails_when_shellcheck_cannot_run() {
    let repo = TestRepo::new("shellcheck-on-missing");
    let file = write_workflow(&repo, "unquoted", UNQUOTED_SCRIPT);
    let run = lint(
        &repo,
        &file,
        &[
            "--shellcheck",
            "on",
            "--shellcheck-path",
            "/nonexistent/shellcheck",
        ],
    );
    run.output.assert_failure();
    assert!(
        run.output.stderr.to_lowercase().contains("shellcheck"),
        "stderr must explain the missing shellcheck: {}",
        run.output.stderr
    );
}

#[test]
fn unknown_mode_names_the_accepted_values() {
    let repo = TestRepo::new("shellcheck-bogus-mode");
    let file = write_workflow(&repo, "unquoted", UNQUOTED_SCRIPT);
    let run = lint(&repo, &file, &["--shellcheck", "bogus"]);
    run.output.assert_failure();
    let stderr = run.output.stderr.to_lowercase();
    assert!(stderr.contains("auto"), "stderr: {}", run.output.stderr);
    assert!(stderr.contains("on"), "stderr: {}", run.output.stderr);
    assert!(stderr.contains("off"), "stderr: {}", run.output.stderr);
}

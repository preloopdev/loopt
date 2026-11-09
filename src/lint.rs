//! `loopt lint`: native correctness and security review of workflow YAML.
//!
//! The checks come from the shared MIT `preloop-gha-lint` and
//! `preloop-gha-sec` crates, plus an optional external `shellcheck` process
//! for `run:` scripts. Nothing is downloaded; repository evidence that would
//! need a network call is reported as unavailable rather than treated as
//! safe.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use preloop_gha_authoring::WorkflowDocument;
use preloop_gha_lint::{
    lint_workflow_with_reusable, Diagnostic, LintContext, ReusableWorkflows, Severity,
};
use preloop_gha_sec::{audit, AuditContext, OnlineFacts, Persona, UnavailableCheck};
use serde::Serialize;
use serde_json::Value;

use crate::lint_online;
use crate::manifest::Manifest;
use crate::paths;
use crate::shellcheck::{self, ShellcheckOptions};

const WORKFLOWS_DIR: &str = ".github/workflows";

pub struct LintOptions {
    pub paths: Vec<PathBuf>,
    pub json: bool,
    pub persona: Persona,
    pub context: Option<PathBuf>,
    pub shellcheck: ShellcheckOptions,
    /// Gather GitHub repository facts and run the online audits.
    pub online: bool,
}

#[derive(Debug, Serialize)]
pub struct FileFindings {
    pub file: String,
    pub findings: Vec<Diagnostic>,
    pub unavailable: Vec<UnavailableCheck>,
}

/// Repository evidence gathered by `--online`: the security audits' fact
/// snapshot plus the reusable-workflow callee documents fetched at their
/// referenced refs.
#[derive(Clone, Debug, Default)]
pub struct OnlineEvidence {
    pub facts: OnlineFacts,
    pub reusable: ReusableWorkflows,
}

pub fn run(root: &Path, options: LintOptions) -> Result<()> {
    let paths = discover_paths(root, &options.paths)?;
    if paths.is_empty() {
        bail!("no workflow YAML to lint (pass paths, or generate workflows first)");
    }
    let context = load_context(root, options.context.as_deref())?;
    let online = if options.online {
        let (facts, reusable) = lint_online::gather(root, &paths)?;
        Some(OnlineEvidence { facts, reusable })
    } else {
        None
    };
    let reports = check_paths(
        root,
        &paths,
        options.persona,
        &context,
        &options.shellcheck,
        online.as_ref(),
    )?;

    if options.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        print_human(&reports);
    }

    let errors = reports
        .iter()
        .flat_map(|report| &report.findings)
        .filter(|finding| finding.severity == Severity::Error)
        .count();
    if errors > 0 {
        bail!("lint reported {errors} error finding(s)");
    }
    Ok(())
}

/// Run the shared checks and shellcheck over `paths` (project-relative or
/// absolute under `root`) without printing or exiting. Used by `loopt gen`
/// after writing.
///
/// Local reusable-workflow callees are read from `root`; remote callees are
/// resolved only when `online` supplies them.
pub fn check_paths(
    root: &Path,
    paths: &[PathBuf],
    persona: Persona,
    context: &LintContext,
    shellcheck_options: &ShellcheckOptions,
    online: Option<&OnlineEvidence>,
) -> Result<Vec<FileFindings>> {
    let audit_context = AuditContext {
        persona,
        known_secret_names: context.available_secrets.clone(),
        online: online.map(|evidence| evidence.facts.clone()),
        ..Default::default()
    };

    let mut documents = Vec::with_capacity(paths.len());
    for path in paths {
        let absolute = root.join(path);
        let text = std::fs::read_to_string(&absolute)
            .with_context(|| format!("read {}", path.display()))?;
        let document =
            WorkflowDocument::parse(&text).with_context(|| format!("parse {}", path.display()))?;
        documents.push((path, document));
    }

    let mut reusable = online
        .map(|evidence| evidence.reusable.clone())
        .unwrap_or_default();
    collect_local_callees(root, &documents, &mut reusable.documents);

    let mut reports = Vec::with_capacity(paths.len());
    for (path, document) in &documents {
        let mut findings = lint_workflow_with_reusable(document, context, Some(&reusable));
        let security = audit(document, &audit_context);
        findings.extend(security.findings);
        findings.extend(shellcheck::check(document, shellcheck_options)?);
        findings.sort_by(|left, right| {
            (&left.path, &left.rule_id, &left.source, &left.message).cmp(&(
                &right.path,
                &right.rule_id,
                &right.source,
                &right.message,
            ))
        });
        findings.dedup();
        let mut unavailable = security.unavailable;
        unavailable.extend(reusable_unavailable(
            document.value(),
            &reusable.unavailable,
        ));
        reports.push(FileFindings {
            file: path.display().to_string(),
            findings,
            unavailable,
        });
    }
    Ok(reports)
}

/// Read every local reusable-workflow callee referenced by `documents` into
/// `callees`, keyed by the literal `uses:` value. A file that cannot be read
/// stays absent, which the lint crate reports as a missing callee.
fn collect_local_callees(
    root: &Path,
    documents: &[(&PathBuf, WorkflowDocument)],
    callees: &mut BTreeMap<String, String>,
) {
    for (_, document) in documents {
        let Some(jobs) = document.value().get("jobs").and_then(Value::as_object) else {
            continue;
        };
        for job in jobs.values() {
            let Some(Value::String(uses)) = job.get("uses") else {
                continue;
            };
            if uses.contains("${{") || uses.contains('@') {
                continue;
            }
            if callees.contains_key(uses) {
                continue;
            }
            let Some(relative) = uses.strip_prefix("./").or_else(|| uses.strip_prefix("$/")) else {
                continue;
            };
            if relative.is_empty() {
                continue;
            }
            let Ok(path) = paths::resolve_within(root, Path::new(relative)) else {
                continue;
            };
            if let Ok(source) = std::fs::read_to_string(&path) {
                callees.insert(uses.clone(), source);
            }
        }
    }
}

/// Report remote reusable-workflow calls whose callee document the online
/// resolver could not fetch, so they are unavailable rather than clean.
fn reusable_unavailable(
    document: &Value,
    unresolved: &BTreeMap<String, String>,
) -> Vec<UnavailableCheck> {
    let mut checks = Vec::new();
    let Some(jobs) = document.get("jobs").and_then(Value::as_object) else {
        return checks;
    };
    for (job_id, job) in jobs {
        let Some(Value::String(uses)) = job.get("uses") else {
            continue;
        };
        let Some(reason) = unresolved.get(uses) else {
            continue;
        };
        checks.push(UnavailableCheck {
            rule_id: "preloop/reusable-workflow-call".to_owned(),
            path: format!(
                "/jobs/{}/uses",
                job_id.replace('~', "~0").replace('/', "~1")
            ),
            reason: reason.clone(),
        });
    }
    checks
}

pub fn print_human(reports: &[FileFindings]) {
    for report in reports {
        for finding in &report.findings {
            let severity = match finding.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "info",
            };
            let location = match (finding.line, finding.column) {
                (Some(line), Some(column)) => format!("{}:{line}:{column}", report.file),
                (Some(line), None) => format!("{}:{line}", report.file),
                _ => report.file.clone(),
            };
            eprintln!(
                "{location}: {severity} [{}] {}",
                finding.rule_id, finding.message
            );
            if let Some(help) = &finding.help {
                eprintln!("  help: {help}");
            }
        }
        if !report.unavailable.is_empty() {
            eprintln!(
                "{}: {} security check(s) need repository evidence and were not run \
                 (rerun with --online to fetch it)",
                report.file,
                report.unavailable.len()
            );
        }
    }
}

fn load_context(root: &Path, path: Option<&Path>) -> Result<LintContext> {
    let default_path = Path::new(".loopt/secrets.json");
    let path = match path {
        Some(path) => path,
        None if root.join(default_path).is_file() => default_path,
        None => return Ok(LintContext::default()),
    };
    let path = paths::resolve_within(root, &paths::relativize(root, path)?)
        .with_context(|| format!("resolve lint context {}", path.display()))?;
    let bytes =
        std::fs::read(&path).with_context(|| format!("read lint context {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse lint context {}", path.display()))
}

fn discover_paths(root: &Path, explicit: &[PathBuf]) -> Result<Vec<PathBuf>> {
    if !explicit.is_empty() {
        let mut found = Vec::new();
        for path in explicit {
            let relative = paths::relativize(root, path)?;
            paths::resolve_within(root, &relative)
                .with_context(|| format!("{}: unsafe path", relative.display()))?;
            if !root.join(&relative).is_file() {
                bail!("{}: no such file", relative.display());
            }
            found.push(relative);
        }
        found.sort();
        found.dedup();
        return Ok(found);
    }

    let manifest = Manifest::load(root)?;
    let mut found: BTreeSet<PathBuf> = manifest
        .entries
        .iter()
        .map(|entry| entry.output.clone())
        .collect();
    if found.is_empty() {
        let dir = root.join(WORKFLOWS_DIR);
        if dir.is_dir() {
            for entry in
                std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))?
            {
                let entry = entry?;
                let path = entry.path();
                if path.is_file()
                    && matches!(
                        path.extension().and_then(|ext| ext.to_str()),
                        Some("yml" | "yaml")
                    )
                {
                    found.insert(
                        path.strip_prefix(root)
                            .expect("workflow path is under root")
                            .to_path_buf(),
                    );
                }
            }
        }
    }
    Ok(found.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use preloop_gha_sec::{OnlineFacts, RepoFacts};

    use super::*;

    const IMPOSTOR: &str = "c7d749a2d57b4b375d1ebcd17cfbfb60c676f18e";

    fn scratch_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "loopt-lint-online-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    fn workflow_with_impostor() -> String {
        format!(
            "name: ci\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@{IMPOSTOR}\n"
        )
    }

    /// Fixture facts must flow from `check_paths` into the online audits with
    /// no network access: the impostor finding is produced and nothing is
    /// reported as unavailable.
    #[test]
    fn fixture_online_facts_drive_audits_without_network() {
        let dir = scratch_dir("fixture");
        let workflow = dir.join("ci.yml");
        std::fs::write(&workflow, workflow_with_impostor()).expect("write workflow");

        let facts = OnlineFacts {
            repos: BTreeMap::from([(
                "actions/checkout".to_owned(),
                RepoFacts {
                    commit_membership: BTreeMap::from([(IMPOSTOR.to_owned(), false)]),
                    archived: Some(false),
                    advisories: Some(Vec::new()),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        };

        let evidence = OnlineEvidence {
            facts,
            reusable: ReusableWorkflows::default(),
        };
        let reports = check_paths(
            &dir,
            &[PathBuf::from("ci.yml")],
            Persona::Regular,
            &LintContext::default(),
            &ShellcheckOptions::default(),
            Some(&evidence),
        )
        .expect("lint fixture workflow");

        assert!(
            reports[0].unavailable.is_empty(),
            "{:?}",
            reports[0].unavailable
        );
        let impostor = reports[0]
            .findings
            .iter()
            .find(|finding| finding.rule_id == "impostor-commit")
            .expect("expected impostor-commit finding");
        assert_eq!(impostor.severity, Severity::Error);

        std::fs::remove_dir_all(&dir).expect("remove scratch dir");
    }

    /// Without a snapshot the same workflow is unknown, not clean: the audit
    /// stays unavailable, which is the offline default's contract.
    #[test]
    fn offline_without_facts_reports_unavailable() {
        let dir = scratch_dir("offline");
        let workflow = dir.join("ci.yml");
        std::fs::write(&workflow, workflow_with_impostor()).expect("write workflow");

        let reports = check_paths(
            &dir,
            &[PathBuf::from("ci.yml")],
            Persona::Regular,
            &LintContext::default(),
            &ShellcheckOptions::default(),
            None,
        )
        .expect("lint fixture workflow");

        assert!(
            reports[0]
                .unavailable
                .iter()
                .any(|check| check.rule_id == "impostor-commit"),
            "{:?}",
            reports[0].unavailable
        );

        std::fs::remove_dir_all(&dir).expect("remove scratch dir");
    }

    /// A local callee is read from the lint root and its declaration is
    /// checked without any network access.
    #[test]
    fn local_reusable_callee_is_checked_from_the_repository_root() {
        let dir = scratch_dir("reusable-local");
        let workflows = dir.join(".github/workflows");
        std::fs::create_dir_all(&workflows).expect("create workflows dir");
        std::fs::write(
            workflows.join("callee.yml"),
            "on:\n  workflow_call:\n    inputs:\n      name:\n        type: string\n        required: true\n",
        )
        .expect("write callee");

        for uses in [
            "./.github/workflows/callee.yml",
            "$/.github/workflows/callee.yml",
        ] {
            std::fs::write(
                dir.join("caller.yml"),
                format!(
                    "name: ci\non: push\njobs:\n  call:\n    uses: {uses}\n    with:\n      bogus: x\n"
                ),
            )
            .expect("write caller");

            let reports = check_paths(
                &dir,
                &[PathBuf::from("caller.yml")],
                Persona::Regular,
                &LintContext::default(),
                &ShellcheckOptions::default(),
                None,
            )
            .expect("lint caller");

            let rules: Vec<&str> = reports[0]
                .findings
                .iter()
                .map(|finding| finding.rule_id.as_str())
                .collect();
            assert!(
                rules.contains(&"preloop/reusable-unknown-input"),
                "{uses}: {rules:?}"
            );
            assert!(
                rules.contains(&"preloop/reusable-missing-input"),
                "{uses}: {rules:?}"
            );
        }

        std::fs::remove_dir_all(&dir).expect("remove scratch dir");
    }

    /// A remote callee supplied by the online resolver reaches the same
    /// checks through `check_paths`, with no network access in the test.
    #[test]
    fn fixture_online_callee_is_checked_without_network() {
        let dir = scratch_dir("reusable-online");
        let uses = "owner/repo/.github/workflows/callee.yml@v1";
        std::fs::write(
            dir.join("caller.yml"),
            format!(
                "name: ci\non: push\njobs:\n  call:\n    uses: {uses}\n    with:\n      bogus: x\n"
            ),
        )
        .expect("write caller");

        let evidence = OnlineEvidence {
            facts: OnlineFacts::default(),
            reusable: ReusableWorkflows {
                documents: BTreeMap::from([(
                    uses.to_owned(),
                    "on:\n  workflow_call:\n    inputs:\n      name:\n        type: string\n        required: true\n"
                        .to_owned(),
                )]),
                unavailable: BTreeMap::new(),
            },
        };

        let reports = check_paths(
            &dir,
            &[PathBuf::from("caller.yml")],
            Persona::Regular,
            &LintContext::default(),
            &ShellcheckOptions::default(),
            Some(&evidence),
        )
        .expect("lint caller");

        let rules: Vec<&str> = reports[0]
            .findings
            .iter()
            .map(|finding| finding.rule_id.as_str())
            .collect();
        assert!(
            rules.contains(&"preloop/reusable-unknown-input"),
            "{rules:?}"
        );
        assert!(
            rules.contains(&"preloop/reusable-missing-input"),
            "{rules:?}"
        );

        std::fs::remove_dir_all(&dir).expect("remove scratch dir");
    }

    /// A remote callee the online resolver could not fetch is reported as
    /// unavailable, never as clean.
    #[test]
    fn unresolved_online_callee_is_unavailable() {
        let dir = scratch_dir("reusable-unavailable");
        let uses = "owner/repo/.github/workflows/callee.yml@v1";
        std::fs::write(
            dir.join("caller.yml"),
            format!("name: ci\non: push\njobs:\n  call:\n    uses: {uses}\n"),
        )
        .expect("write caller");

        let evidence = OnlineEvidence {
            facts: OnlineFacts::default(),
            reusable: ReusableWorkflows {
                documents: BTreeMap::new(),
                unavailable: BTreeMap::from([(
                    uses.to_owned(),
                    "gh api failed with HTTP 404".to_owned(),
                )]),
            },
        };

        let reports = check_paths(
            &dir,
            &[PathBuf::from("caller.yml")],
            Persona::Regular,
            &LintContext::default(),
            &ShellcheckOptions::default(),
            Some(&evidence),
        )
        .expect("lint caller");

        assert!(
            reports[0]
                .unavailable
                .iter()
                .any(|check| check.rule_id == "preloop/reusable-workflow-call"
                    && check.path == "/jobs/call/uses"),
            "{:?}",
            reports[0].unavailable
        );

        std::fs::remove_dir_all(&dir).expect("remove scratch dir");
    }
}

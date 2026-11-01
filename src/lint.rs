//! `loopt lint`: native correctness and security review of workflow YAML.
//!
//! The checks come from the shared MIT `preloop-gha-lint` and
//! `preloop-gha-sec` crates, plus an optional external `shellcheck` process
//! for `run:` scripts. Nothing is downloaded; repository evidence that would
//! need a network call is reported as unavailable rather than treated as
//! safe.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use preloop_gha_authoring::WorkflowDocument;
use preloop_gha_lint::{lint_workflow, Diagnostic, LintContext, Severity};
use preloop_gha_sec::{audit, AuditContext, Persona, UnavailableCheck};
use serde::Serialize;

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
}

#[derive(Debug, Serialize)]
pub struct FileFindings {
    pub file: String,
    pub findings: Vec<Diagnostic>,
    pub unavailable: Vec<UnavailableCheck>,
}

pub fn run(root: &Path, options: LintOptions) -> Result<()> {
    let paths = discover_paths(root, &options.paths)?;
    if paths.is_empty() {
        bail!("no workflow YAML to lint (pass paths, or generate workflows first)");
    }
    let context = load_context(root, options.context.as_deref())?;
    let reports = check_paths(root, &paths, options.persona, &context, &options.shellcheck)?;

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
pub fn check_paths(
    root: &Path,
    paths: &[PathBuf],
    persona: Persona,
    context: &LintContext,
    shellcheck_options: &ShellcheckOptions,
) -> Result<Vec<FileFindings>> {
    let mut reports = Vec::with_capacity(paths.len());
    for path in paths {
        let absolute = root.join(path);
        let text = std::fs::read_to_string(&absolute)
            .with_context(|| format!("read {}", path.display()))?;
        let document =
            WorkflowDocument::parse(&text).with_context(|| format!("parse {}", path.display()))?;
        let mut findings = lint_workflow(&document, context);
        let security = audit(
            &document,
            &AuditContext {
                persona,
                known_secret_names: context.available_secrets.clone(),
                ..Default::default()
            },
        );
        findings.extend(security.findings);
        findings.extend(shellcheck::check(&document, shellcheck_options)?);
        findings.sort_by(|left, right| {
            (&left.path, &left.rule_id, &left.source, &left.message).cmp(&(
                &right.path,
                &right.rule_id,
                &right.source,
                &right.message,
            ))
        });
        findings.dedup();
        reports.push(FileFindings {
            file: path.display().to_string(),
            findings,
            unavailable: security.unavailable,
        });
    }
    Ok(reports)
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
                "{}: {} security check(s) need repository evidence and were not run",
                report.file,
                report.unavailable.len()
            );
        }
    }
}

fn load_context(root: &Path, path: Option<&Path>) -> Result<LintContext> {
    let Some(path) = path else {
        return Ok(LintContext::default());
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

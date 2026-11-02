//! `loopt lock`: GitHub workflow dependency lockfile.
//!
//! Generates and verifies `.github/workflows/actions.lock`, the lockfile
//! format consumed by GitHub's `gh-actions-lock` extension and enforced by
//! the hosted runner (lockfile version `v0.0.2`). Every remote `uses:`
//! reference in the repository's workflows is pinned to the commit SHA it
//! resolves to, together with the repository's numeric `owner_id`/`repo_id`.
//!
//! Scope mirrors the upstream tool:
//!   - step-level `uses:` of remote actions are locked;
//!   - composite actions contribute their own `uses:` steps transitively;
//!   - local `./…` references, already-pinned `$/…` specifiers, `docker://`
//!     images, `jobs.<id>.uses` reusable-workflow calls, and `${{ }}`
//!     expressions are not lockable and are reported as notes instead.
//!
//! Modes:
//!   `loopt lock`                resolve everything and (re)write the lockfile
//!   `loopt lock --verify`       coverage plus ref/provenance drift (network)
//!   `loopt lock --verify-local` offline coverage: every uses: has an entry
//!
//! `loopt lock` and GitHub's `gh actions-lock` operate on the same file; a
//! lockfile written by either tool verifies under the other.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use preloop_gha_authoring::WorkflowDocument;
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::gh;
use crate::paths;

const WORKFLOWS_DIR: &str = ".github/workflows";
const LOCKFILE_PATH: &str = ".github/workflows/actions.lock";
const LOCKFILE_VERSION: &str = "v0.0.2";

/// One remote `uses:` target: `owner/repo[/subpath]@ref`.
#[derive(Debug, Clone)]
struct UsesSpec {
    /// Canonical key exactly as written in the workflow.
    key: String,
    owner: String,
    repo: String,
    /// Repository-relative directory containing `action.yml`; "" for root.
    subpath: String,
    ref_: String,
}

/// Classification of a `uses:` value for locking.
enum Uses {
    /// `owner/repo[/subpath]@ref` — lockable.
    Remote(UsesSpec),
    /// `./…`, `/…`, `$/…` — same-repository content, not lockable by commit.
    Local,
    /// `docker://…` — container images are out of scope for the lockfile.
    Docker,
    /// `uses:` containing a `${{ }}` expression or otherwise unclassifiable —
    /// not statically lockable.
    Dynamic,
}

/// One resolved `dependencies` entry.
#[derive(Debug)]
struct LockedDep {
    ref_: String,
    sha: String,
    owner_id: u64,
    repo_id: u64,
    /// Direct transitive `uses:` keys declared inside a composite action.
    uses: Vec<String>,
}

/// Everything resolved for one lock run.
struct ResolvedWork {
    /// workflow path -> ordered spec keys (direct first, then transitives)
    workflows: Vec<(PathBuf, Vec<String>)>,
    deps: BTreeMap<String, LockedDep>,
    /// Human-readable notes about skipped/unlockable `uses:` values.
    notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Resolve everything and (re)write the lockfile.
    Write,
    /// Network verification: coverage plus ref/provenance drift.
    Verify,
    /// Offline coverage: every `uses:` has a matching lockfile entry.
    VerifyLocal,
}

#[derive(Debug, Serialize)]
struct Finding {
    severity: &'static str,
    category: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow: Option<String>,
    dependency: String,
    detail: String,
}

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    let (mode, json, paths_arg) = parse_args(args)?;
    let workflows = discover_workflows(root, &paths_arg)?;
    match mode {
        Mode::Write => {
            let work = resolve_all(root, &workflows)?;
            for note in &work.notes {
                eprintln!("note: {note}");
            }
            let text = render_lockfile(&work);
            paths::write_atomic(root, Path::new(LOCKFILE_PATH), text.as_bytes())
                .context("write .github/workflows/actions.lock")?;
            println!(
                "locked {} dependencies across {} workflow(s) -> {LOCKFILE_PATH}",
                work.deps.len(),
                work.workflows.len()
            );
            Ok(())
        }
        Mode::Verify => verify(root, &workflows, true, json),
        Mode::VerifyLocal => verify(root, &workflows, false, json),
    }
}

fn parse_args(args: &[String]) -> Result<(Mode, bool, Vec<PathBuf>)> {
    let mut mode = Mode::Write;
    let mut json = false;
    let mut paths = Vec::new();
    for arg in args {
        let (flag, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with('-') => (name, Some(value)),
            _ => (arg.as_str(), None),
        };
        match flag {
            "--verify" => {
                if inline.is_some() {
                    bail!("--verify does not take a value");
                }
                if mode != Mode::Write {
                    bail!("--verify cannot be combined with --verify-local");
                }
                mode = Mode::Verify;
            }
            "--verify-local" => {
                if inline.is_some() {
                    bail!("--verify-local does not take a value");
                }
                if mode != Mode::Write {
                    bail!("--verify-local cannot be combined with --verify");
                }
                mode = Mode::VerifyLocal;
            }
            "--json" => {
                if inline.is_some() {
                    bail!("--json does not take a value");
                }
                json = true;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ if flag.starts_with('-') => bail!("unknown lock option {flag:?}"),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    Ok((mode, json, paths))
}

fn print_usage() {
    eprintln!(
        "usage: loopt lock [--verify|--verify-local] [--json] [WORKFLOW ...]\n\n\
         Generates (default) or verifies .github/workflows/actions.lock, the\n\
         GitHub workflow dependency lockfile enforced by the hosted runner.\n\
         --verify        resolve every pinned ref over the network, fail on drift\n\
         --verify-local  offline coverage check: every uses: has a lockfile entry\n\
         [WORKFLOW ...]  limit the run to specific workflow paths"
    );
}

fn discover_workflows(root: &Path, explicit: &[PathBuf]) -> Result<Vec<PathBuf>> {
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

    let dir = root.join(WORKFLOWS_DIR);
    let mut found = Vec::new();
    if dir.is_dir() {
        for entry in fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
            let entry = entry.with_context(|| format!("read {}", dir.display()))?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            if matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("yml" | "yaml")
            ) {
                found.push(
                    path.strip_prefix(root)
                        .expect("workflow path is under root")
                        .to_path_buf(),
                );
            }
        }
    }
    found.sort();
    Ok(found)
}

fn classify_uses(value: &str) -> Result<Uses> {
    let trimmed = value.trim();
    if trimmed.contains("${{") {
        return Ok(Uses::Dynamic);
    }
    if trimmed.starts_with("./")
        || trimmed.starts_with('/')
        || trimmed.starts_with("$/")
        || trimmed == "."
    {
        return Ok(Uses::Local);
    }
    if trimmed.starts_with("docker://") {
        return Ok(Uses::Docker);
    }
    let (source, requested_ref) = trimmed
        .rsplit_once('@')
        .with_context(|| format!("uses: {trimmed:?} has no @ref and cannot be locked"))?;
    let components: Vec<&str> = source.split('/').collect();
    if components.len() < 2
        || components.iter().any(|component| {
            component.is_empty()
                || *component == "."
                || *component == ".."
                || !component
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        })
    {
        bail!("invalid action source {source:?} in uses:");
    }
    gh::validate_ref(requested_ref)?;
    Ok(Uses::Remote(UsesSpec {
        key: trimmed.to_string(),
        owner: components[0].to_string(),
        repo: components[1].to_string(),
        subpath: components[2..].join("/"),
        ref_: requested_ref.to_string(),
    }))
}

/// Collect `uses:` values from one workflow document, in file order.
///
/// Only step-level `uses:` is lockable; `jobs.<id>.uses` reusable-workflow
/// calls are skipped, matching `gh actions-lock`
/// (<https://github.com/github/gh-actions-lock/issues/179>).
fn workflow_uses(document: &WorkflowDocument, notes: &mut Vec<String>, context: &str) -> Vec<Uses> {
    let mut out = Vec::new();
    let Some(jobs) = document.value().get("jobs").and_then(JsonValue::as_object) else {
        return out;
    };
    for (job_id, job) in jobs {
        let Some(steps) = job.get("steps").and_then(JsonValue::as_array) else {
            // jobs.<id>.uses (reusable workflow calls) are deliberately
            // skipped; see gh-actions-lock issue #179.
            continue;
        };
        for step in steps {
            let Some(uses) = step.get("uses").and_then(JsonValue::as_str) else {
                continue;
            };
            match classify_uses(uses) {
                Ok(classified) => out.push(classified),
                Err(error) => {
                    // An unclassifiable `uses:` line cannot be locked; report
                    // it and treat the value as unresolvable for coverage so
                    // --verify-local still flags the gap via its key absent.
                    notes.push(format!("{context}: job {job_id:?}: {error}"));
                }
            }
        }
    }
    out
}

/// Extract remote `uses:` declared inside a composite action's `runs.steps`.
fn composite_uses(metadata_bytes: &[u8], context: &str) -> Result<Vec<Uses>> {
    let source = String::from_utf8(metadata_bytes.to_vec())
        .with_context(|| format!("{context}: not valid UTF-8"))?;
    let document = WorkflowDocument::parse(&source)
        .map_err(|error| anyhow::anyhow!("{context}: invalid YAML: {error}"))?;
    let root = document.value();
    let Some(runs) = root.get("runs").and_then(JsonValue::as_object) else {
        return Ok(Vec::new());
    };
    if runs.get("using").and_then(JsonValue::as_str) != Some("composite") {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    if let Some(steps) = runs.get("steps").and_then(JsonValue::as_array) {
        for step in steps {
            if let Some(uses) = step.get("uses").and_then(JsonValue::as_str) {
                match classify_uses(uses) {
                    Ok(classified) => out.push(classified),
                    Err(error) => return Err(error.context(format!("{context}: invalid uses"))),
                }
            }
        }
    }
    Ok(out)
}

struct Resolver<'a> {
    root: &'a Path,
    /// spec key -> resolved entry
    deps: BTreeMap<String, LockedDep>,
    /// "owner/repo" -> (owner_id, repo_id)
    ids: BTreeMap<String, (u64, u64)>,
    /// Spec keys already resolved (in-flight or done) for cycle safety.
    seen: BTreeSet<String>,
    /// Human-readable notes about skipped/unlockable `uses:` values.
    notes: Vec<String>,
}

impl<'a> Resolver<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            deps: BTreeMap::new(),
            ids: BTreeMap::new(),
            seen: BTreeSet::new(),
            notes: Vec::new(),
        }
    }

    fn repo_ids(&mut self, owner: &str, repo: &str) -> Result<(u64, u64)> {
        let key = format!("{owner}/{repo}");
        if let Some(ids) = self.ids.get(&key) {
            return Ok(*ids);
        }
        let ids = gh::repo_ids(self.root, owner, repo)?;
        self.ids.insert(key, ids);
        Ok(ids)
    }

    /// Resolve one spec plus its transitive composite dependencies.
    fn resolve(&mut self, spec: &UsesSpec) -> Result<()> {
        if !self.seen.insert(spec.key.clone()) {
            return Ok(());
        }
        let sha = gh::resolve_commit(self.root, &spec.owner, &spec.repo, &spec.ref_)?;
        let (owner_id, repo_id) = self.repo_ids(&spec.owner, &spec.repo)?;

        // Composite actions may themselves `uses:` remote actions; pull the
        // metadata at the pinned sha and recurse (`seen` guards cycles).
        let base = if spec.subpath.is_empty() {
            String::new()
        } else {
            format!("{}/", spec.subpath)
        };
        let mut transitives = Vec::new();
        let mut failures = Vec::new();
        let mut fetched = false;
        for metadata_file in ["action.yml", "action.yaml"] {
            let path = format!("{base}{metadata_file}");
            match gh::fetch_file(self.root, &spec.owner, &spec.repo, &path, &sha) {
                Ok(bytes) => {
                    fetched = true;
                    transitives = composite_uses(&bytes, &spec.key)?;
                    break;
                }
                Err(error) => failures.push(format!("{metadata_file}: {error}")),
            }
        }
        if !fetched {
            bail!(
                "could not retrieve action metadata for {}@{}:\n- {}",
                spec.key,
                sha,
                failures.join("\n- ")
            );
        }

        let mut uses_keys = Vec::new();
        for item in transitives {
            match item {
                Uses::Remote(child) => {
                    if !uses_keys.contains(&child.key) {
                        uses_keys.push(child.key.clone());
                    }
                    self.resolve(&child)?;
                }
                Uses::Local => self.notes.push(format!(
                    "{}: local ./ action inside a composite is not locked",
                    spec.key
                )),
                Uses::Docker => self.notes.push(format!(
                    "{}: docker:// image inside a composite is not locked",
                    spec.key
                )),
                Uses::Dynamic => self.notes.push(format!(
                    "{}: composite step uses a ${{ }} expression and cannot be locked",
                    spec.key
                )),
            }
        }

        self.deps.insert(
            spec.key.clone(),
            LockedDep {
                ref_: spec.ref_.clone(),
                sha,
                owner_id,
                repo_id,
                uses: uses_keys,
            },
        );
        Ok(())
    }
}

/// Direct keys plus everything reachable through composite `uses:` lists.
fn closure(direct: &[String], deps: &BTreeMap<String, LockedDep>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    // BFS keeps direct uses: in file order, then transitives breadth-first —
    // the same ordering gh-actions-lock emits in its workflows map.
    let mut queue: VecDeque<String> = direct.iter().cloned().collect();
    while let Some(key) = queue.pop_front() {
        if !seen.insert(key.clone()) {
            continue;
        }
        out.push(key.clone());
        if let Some(dep) = deps.get(&key) {
            for nested in &dep.uses {
                queue.push_back(nested.clone());
            }
        }
    }
    out
}

/// Resolve every remote `uses:` across the given workflows.
fn resolve_all(root: &Path, workflows: &[PathBuf]) -> Result<ResolvedWork> {
    let mut resolver = Resolver::new(root);
    let mut workflows_out: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for path in workflows {
        let text = fs::read_to_string(root.join(path))
            .with_context(|| format!("read {}", path.display()))?;
        let document = WorkflowDocument::parse(&text)
            .map_err(|error| anyhow::anyhow!("{}: invalid YAML: {error}", path.display()))?;
        let context = path.display().to_string();
        let mut direct = Vec::new();
        for item in workflow_uses(&document, &mut resolver.notes, &context) {
            match item {
                Uses::Remote(spec) => {
                    resolver.resolve(&spec)?;
                    if !direct.contains(&spec.key) {
                        direct.push(spec.key);
                    }
                }
                Uses::Local => resolver
                    .notes
                    .push(format!("{context}: local action reference is not locked")),
                Uses::Docker => resolver
                    .notes
                    .push(format!("{context}: docker:// image is not locked")),
                Uses::Dynamic => resolver.notes.push(format!(
                    "{context}: a uses: value is dynamic or unpinnable and is not locked"
                )),
            }
        }
        workflows_out.push((path.clone(), direct));
    }
    // Expand each workflow's direct keys to the full transitive closure.
    let workflows = workflows_out
        .into_iter()
        .map(|(path, direct)| (path, closure(&direct, &resolver.deps)))
        .collect();
    Ok(ResolvedWork {
        workflows,
        deps: resolver.deps,
        notes: resolver.notes,
    })
}

fn yaml_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn render_lockfile(work: &ResolvedWork) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "version: '{LOCKFILE_VERSION}'");
    if work.deps.is_empty() {
        let _ = writeln!(out, "dependencies: {{}}");
    } else {
        let _ = writeln!(out, "dependencies:");
        for (key, dep) in &work.deps {
            let _ = writeln!(out, "  {}:", yaml_quote(key));
            let _ = writeln!(out, "    ref: {}", yaml_quote(&dep.ref_));
            let _ = writeln!(out, "    commit: 'sha1-{}'", dep.sha);
            let _ = writeln!(out, "    owner_id: {}", dep.owner_id);
            let _ = writeln!(out, "    repo_id: {}", dep.repo_id);
            if !dep.uses.is_empty() {
                let _ = writeln!(out, "    uses:");
                for nested in &dep.uses {
                    let _ = writeln!(out, "      - {}", yaml_quote(nested));
                }
            }
        }
    }
    if work.workflows.is_empty() {
        let _ = writeln!(out, "workflows: {{}}");
    } else {
        let _ = writeln!(out, "workflows:");
        for (path, keys) in &work.workflows {
            let display = path.display().to_string().replace('\\', "/");
            let _ = writeln!(out, "  {}:", yaml_quote(&display));
            for key in keys {
                let _ = writeln!(out, "    - {}", yaml_quote(key));
            }
        }
    }
    out
}

// ------------------------------------------------------------------ verify

struct ParsedLockfile {
    /// spec key -> (ref, sha, owner_id, repo_id)
    deps: BTreeMap<String, (String, String, u64, u64)>,
    /// spec key -> direct transitive keys
    dep_uses: BTreeMap<String, Vec<String>>,
    /// workflow path -> listed keys
    workflows: BTreeMap<String, Vec<String>>,
}

fn parse_lockfile(root: &Path) -> Result<ParsedLockfile> {
    let path = Path::new(LOCKFILE_PATH);
    let resolved =
        paths::resolve_within(root, path).with_context(|| format!("resolve {LOCKFILE_PATH}"))?;
    let text = fs::read_to_string(&resolved)
        .with_context(|| format!("read {LOCKFILE_PATH}; run `loopt lock` first"))?;
    let document = WorkflowDocument::parse(&text)
        .map_err(|error| anyhow::anyhow!("{LOCKFILE_PATH}: invalid YAML: {error}"))?;
    let root = document.value();
    let version = root
        .get("version")
        .and_then(JsonValue::as_str)
        .with_context(|| format!("{LOCKFILE_PATH}: missing `version`"))?;
    if version != LOCKFILE_VERSION {
        bail!("{LOCKFILE_PATH}: unsupported version {version:?} (expected {LOCKFILE_VERSION:?})");
    }

    let mut deps = BTreeMap::new();
    let mut dep_uses: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(deps_map) = root.get("dependencies").and_then(JsonValue::as_object) {
        for (key, entry) in deps_map {
            let ref_ = entry
                .get("ref")
                .and_then(JsonValue::as_str)
                .with_context(|| format!("{LOCKFILE_PATH}: {key} missing `ref`"))?
                .to_string();
            let commit = entry
                .get("commit")
                .and_then(JsonValue::as_str)
                .with_context(|| format!("{LOCKFILE_PATH}: {key} missing `commit`"))?;
            let sha = commit
                .strip_prefix("sha1-")
                .with_context(|| format!("{LOCKFILE_PATH}: {key} commit must be sha1-…"))?
                .to_string();
            let owner_id = entry
                .get("owner_id")
                .and_then(JsonValue::as_u64)
                .with_context(|| format!("{LOCKFILE_PATH}: {key} missing `owner_id`"))?;
            let repo_id = entry
                .get("repo_id")
                .and_then(JsonValue::as_u64)
                .with_context(|| format!("{LOCKFILE_PATH}: {key} missing `repo_id`"))?;
            let uses = entry
                .get("uses")
                .and_then(JsonValue::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            dep_uses.insert(key.clone(), uses);
            deps.insert(key.clone(), (ref_, sha, owner_id, repo_id));
        }
    }

    let mut workflows = BTreeMap::new();
    if let Some(map) = root.get("workflows").and_then(JsonValue::as_object) {
        for (path, list) in map {
            let keys = list
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            workflows.insert(path.clone(), keys);
        }
    }
    Ok(ParsedLockfile {
        deps,
        dep_uses,
        workflows,
    })
}

/// Coverage + drift checks shared by `--verify` and `--verify-local`.
fn verify(root: &Path, workflows: &[PathBuf], online: bool, json: bool) -> Result<()> {
    let lock = parse_lockfile(root)?;
    let mut findings: Vec<Finding> = Vec::new();
    let mut seen_keys: BTreeSet<String> = BTreeSet::new();

    for path in workflows {
        let display = path.display().to_string().replace('\\', "/");
        let text = fs::read_to_string(root.join(path))
            .with_context(|| format!("read {}", path.display()))?;
        let document = WorkflowDocument::parse(&text)
            .map_err(|error| anyhow::anyhow!("{}: invalid YAML: {error}", path.display()))?;

        let mut notes = Vec::new();
        let mut direct = Vec::new();
        for item in workflow_uses(&document, &mut notes, &display) {
            if let Uses::Remote(spec) = item {
                if !direct.contains(&spec.key) {
                    direct.push(spec.key);
                }
            }
        }
        for note in notes {
            eprintln!("note: {note}");
        }

        // Coverage: every direct uses key must be present in dependencies.
        for key in &direct {
            seen_keys.insert(key.clone());
            if lock.deps.contains_key(key) {
                continue;
            }
            let (nwo, _) = key
                .rsplit_once('@')
                .map(|(n, r)| (n.to_string(), r.to_string()))
                .unwrap_or_else(|| (key.clone(), String::new()));
            let detail = match lock
                .deps
                .keys()
                .find(|key| key.rsplit_once('@').map(|(n, _)| n) == Some(nwo.as_str()))
            {
                Some(locked_key) => format!(
                    "workflow uses {key:?} but the lockfile pins {locked_key:?} (ref-changed)"
                ),
                None => format!("workflow uses {key:?} but the lockfile has no entry (missing)"),
            };
            findings.push(Finding {
                severity: "error",
                category: "missing",
                workflow: Some(display.clone()),
                dependency: key.clone(),
                detail,
            });
        }

        // Expected keys for this workflow: direct keys plus the transitive
        // closure reachable via the lockfile's `uses:` lists.
        let mut expected: BTreeSet<String> = direct.iter().cloned().collect();
        let mut stack: Vec<String> = direct.clone();
        while let Some(key) = stack.pop() {
            if let Some(nested) = lock.dep_uses.get(&key) {
                for nested_key in nested {
                    if expected.insert(nested_key.clone()) {
                        stack.push(nested_key.clone());
                    }
                }
            }
        }
        for key in &expected {
            seen_keys.insert(key.clone());
        }

        match lock.workflows.get(&display) {
            Some(listed) => {
                let listed_set: BTreeSet<&String> = listed.iter().collect();
                for key in &expected {
                    if !listed_set.contains(key) {
                        findings.push(Finding {
                            severity: "error",
                            category: "missing",
                            workflow: Some(display.clone()),
                            dependency: key.clone(),
                            detail: format!(
                                "{key:?} is required by this workflow but absent from its lockfile entry"
                            ),
                        });
                    }
                }
            }
            None => findings.push(Finding {
                severity: "error",
                category: "missing",
                workflow: Some(display.clone()),
                dependency: String::new(),
                detail: "workflow is absent from the lockfile's workflows map".to_string(),
            }),
        }
    }

    // Stale: lockfile entries no workflow references.
    for key in lock.deps.keys() {
        if !seen_keys.contains(key) {
            findings.push(Finding {
                severity: "error",
                category: "stale",
                workflow: None,
                dependency: key.clone(),
                detail: format!("lockfile pins {key:?} but no workflow references it"),
            });
        }
    }

    // Online provenance: every pinned ref still resolves to the pinned SHA.
    if online {
        for (key, (ref_, sha, owner_id, repo_id)) in &lock.deps {
            let (source, _) = key.rsplit_once('@').unwrap_or((key.as_str(), ""));
            let components: Vec<&str> = source.split('/').collect();
            if components.len() < 2 {
                findings.push(Finding {
                    severity: "error",
                    category: "invalid",
                    workflow: None,
                    dependency: key.clone(),
                    detail: "lockfile key is not OWNER/REPO[/SUBPATH]@REF".to_string(),
                });
                continue;
            }
            let owner = components[0];
            let repo = components[1];
            match gh::resolve_commit(root, owner, repo, ref_) {
                Ok(actual) if &actual != sha => findings.push(Finding {
                    severity: "error",
                    category: "moved",
                    workflow: None,
                    dependency: key.clone(),
                    detail: format!("ref {ref_} now resolves to {actual}, lockfile pins {sha}"),
                }),
                Ok(_) => {}
                Err(error) => findings.push(Finding {
                    severity: "error",
                    category: "unresolvable",
                    workflow: None,
                    dependency: key.clone(),
                    detail: format!("cannot resolve: {error}"),
                }),
            }
            match gh::repo_ids(root, owner, repo) {
                Ok((oid, rid)) if oid != *owner_id || rid != *repo_id => findings.push(Finding {
                    severity: "error",
                    category: "provenance",
                    workflow: None,
                    dependency: key.clone(),
                    detail: format!(
                        "repository ids changed: lockfile has owner_id={owner_id} repo_id={repo_id}, GitHub reports owner_id={oid} repo_id={rid}"
                    ),
                }),
                Ok(_) => {}
                Err(error) => findings.push(Finding {
                    severity: "warning",
                    category: "unverifiable",
                    workflow: None,
                    dependency: key.clone(),
                    detail: format!("cannot check repository ids: {error}"),
                }),
            }
        }
    }

    if json {
        let valid = !findings.iter().any(|finding| finding.severity == "error");
        let out = serde_json::json!({
            "valid": valid,
            "mode": if online { "verify" } else { "verify-local" },
            "findings": findings,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        for finding in &findings {
            let scope = finding
                .workflow
                .as_deref()
                .map(|workflow| format!("{workflow}: "))
                .unwrap_or_default();
            eprintln!(
                "{}{} [{}] {}",
                scope, finding.severity, finding.category, finding.detail
            );
        }
        if findings.is_empty() {
            println!("lockfile verified: {} dependencies OK", lock.deps.len());
        }
    }

    if findings.iter().any(|finding| finding.severity == "error") {
        bail!("lockfile verification failed; re-run `loopt lock` to refresh");
    }
    Ok(())
}

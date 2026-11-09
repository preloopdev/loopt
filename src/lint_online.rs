//! Gathers the repository facts that `loopt lint --online` feeds to
//! `preloop-gha-sec`'s online audits.
//!
//! All network access goes through [`crate::gh`] — the same authenticated
//! `gh api` plumbing `loopt lock` uses — so `gh auth login`, `GH_HOST`, and
//! the `LOOPT_GH` test override behave identically here. Each repository
//! referenced by a workflow `uses:` is fetched once; the resulting
//! [`OnlineFacts`] snapshot is then handed to the offline audit crate, which
//! never touches the network itself.
//!
//! Only the fields the online audits actually consult are populated:
//! complete tag and branch enumerations, commit-membership answers for
//! hash-pinned refs, the repository's archived flag, and GitHub Security
//! Advisories for the action versions in use. Missing evidence stays
//! missing: a repository we could not inspect aborts the run instead of
//! producing a silently clean result.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use preloop_gha_authoring::WorkflowDocument;
use preloop_gha_lint::ReusableWorkflows;
use preloop_gha_sec::{AdvisoryFact, DatasetProvenance, OnlineFacts, RepoFacts, Severity};
use serde_json::Value;

use crate::gh;

/// One repository referenced by one or more `uses:` clauses.
struct RepoRequest {
    owner: String,
    repo: String,
    refs: BTreeSet<String>,
}

/// A remote reusable-workflow call: `owner/repo/path/to/workflow.yml@ref`.
struct RemoteCall<'a> {
    owner: &'a str,
    repo: &'a str,
    path: &'a str,
    git_ref: &'a str,
}

/// Collect the facts every online audit needs for the workflows in `paths`,
/// plus the callee documents of remote reusable-workflow calls.
///
/// Callee fetches that fail are recorded in [`ReusableWorkflows::unavailable`]
/// rather than aborting the run: the caller reports them as unavailable, never
/// as clean.
pub(crate) fn gather(root: &Path, paths: &[PathBuf]) -> Result<(OnlineFacts, ReusableWorkflows)> {
    let (requests, calls) = collect_repositories(root, paths)?;

    let mut reusable = ReusableWorkflows::default();
    for uses in calls {
        match fetch_reusable(root, &uses) {
            Ok(source) => {
                reusable.documents.insert(uses, source);
            }
            Err(error) => {
                reusable.unavailable.insert(uses, format!("{error:#}"));
            }
        }
    }

    if requests.is_empty() {
        return Ok((OnlineFacts::default(), reusable));
    }

    let mut repos = BTreeMap::new();
    for (key, request) in requests {
        let facts = gather_repo(root, &request)
            .with_context(|| format!("gather GitHub facts for {key}"))?;
        repos.insert(key, facts);
    }

    Ok((
        OnlineFacts {
            provenance: vec![
                DatasetProvenance {
                    dataset: "refs".to_owned(),
                    source: "GitHub REST API via gh".to_owned(),
                    fetched_at: None,
                },
                DatasetProvenance {
                    dataset: "advisories".to_owned(),
                    source: "GitHub global security advisories via gh".to_owned(),
                    fetched_at: None,
                },
            ],
            config: Default::default(),
            repos,
        },
        reusable,
    ))
}

fn collect_repositories(
    root: &Path,
    paths: &[PathBuf],
) -> Result<(BTreeMap<String, RepoRequest>, BTreeSet<String>)> {
    let mut requests: BTreeMap<String, RepoRequest> = BTreeMap::new();
    let mut calls = BTreeSet::new();
    for path in paths {
        let absolute = root.join(path);
        let text = std::fs::read_to_string(&absolute)
            .with_context(|| format!("read {}", path.display()))?;
        let document =
            WorkflowDocument::parse(&text).with_context(|| format!("parse {}", path.display()))?;
        collect_document(document.value(), &mut requests);
        collect_reusable_calls(document.value(), &mut calls);
    }
    Ok((requests, calls))
}

/// Record remote reusable-workflow calls (job-level `uses:`) for callee
/// fetching. Local calls are resolved from the repository by `loopt lint`.
fn collect_reusable_calls(doc: &Value, calls: &mut BTreeSet<String>) {
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for job in jobs.values() {
        if is_disabled(job) {
            continue;
        }
        let Some(Value::String(uses)) = job.get("uses") else {
            continue;
        };
        if parse_remote_call(uses).is_some() {
            calls.insert(uses.clone());
        }
    }
}

fn parse_remote_call(raw: &str) -> Option<RemoteCall<'_>> {
    if raw.contains("${{") {
        return None;
    }
    let (reference, git_ref) = raw.rsplit_once('@')?;
    if reference.starts_with("./")
        || reference.starts_with("../")
        || reference.starts_with('/')
        || reference.starts_with("$/")
        || reference.starts_with("docker://")
        || reference == "."
    {
        return None;
    }
    let mut components = reference.split('/');
    let owner = components.next()?;
    let repo = components.next()?;
    let workflow: Vec<&str> = components.collect();
    if workflow.is_empty()
        || !valid_repo_component(owner)
        || !valid_repo_component(repo)
        || workflow
            .iter()
            .any(|component| component.is_empty() || matches!(*component, "." | ".."))
        || git_ref.is_empty()
        || gh::validate_ref(git_ref).is_err()
    {
        return None;
    }
    let path_start = owner.len() + 1 + repo.len() + 1;
    Some(RemoteCall {
        owner,
        repo,
        path: &reference[path_start..],
        git_ref,
    })
}

fn fetch_reusable(root: &Path, uses: &str) -> Result<String> {
    let call = parse_remote_call(uses)
        .with_context(|| format!("`{uses}` is not a remote reusable workflow call"))?;
    let bytes = gh::fetch_file_at_ref(root, call.owner, call.repo, call.path, call.git_ref)?;
    String::from_utf8(bytes).with_context(|| format!("{uses}: callee workflow is not UTF-8"))
}

fn collect_document(doc: &Value, requests: &mut BTreeMap<String, RepoRequest>) {
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for job in jobs.values() {
        if is_disabled(job) {
            continue;
        }
        if let Some(Value::String(uses)) = job.get("uses") {
            push_uses(uses, requests);
        }
        let Some(steps) = job.get("steps").and_then(Value::as_array) else {
            continue;
        };
        for step in steps {
            if is_disabled(step) {
                continue;
            }
            let Some(Value::String(uses)) = step.get("uses") else {
                continue;
            };
            push_uses(uses, requests);
        }
    }
}

/// Parse a remote `uses:` value and record its repository/ref. Local actions,
/// Docker images, expressions, and anything else that is not a static
/// `owner/repo[/subpath]@ref` reference are ignored.
fn push_uses(raw: &str, requests: &mut BTreeMap<String, RepoRequest>) {
    if raw.contains("${{") {
        return;
    }
    let Some((path, git_ref)) = raw.rsplit_once('@') else {
        return;
    };
    if path.starts_with("./")
        || path.starts_with("../")
        || path.starts_with('/')
        || path.starts_with("$/")
        || path.starts_with("docker://")
        || path == "."
    {
        return;
    }
    let mut components = path.split('/');
    let (Some(owner), Some(repo)) = (components.next(), components.next()) else {
        return;
    };
    if !valid_repo_component(owner) || !valid_repo_component(repo) || git_ref.is_empty() {
        return;
    }
    if gh::validate_ref(git_ref).is_err() {
        return;
    }
    let request = requests
        .entry(format!("{}/{}", owner.to_lowercase(), repo.to_lowercase()))
        .or_insert_with(|| RepoRequest {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            refs: BTreeSet::new(),
        });
    request.refs.insert(git_ref.to_owned());
}

fn valid_repo_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && component
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn gather_repo(root: &Path, request: &RepoRequest) -> Result<RepoFacts> {
    let tags = fetch_refs(root, &request.owner, &request.repo, "tags")?;
    let branches = fetch_refs(root, &request.owner, &request.repo, "branches")?;
    let archived = fetch_archived(root, &request.owner, &request.repo)?;

    let pinned: BTreeSet<String> = request
        .refs
        .iter()
        .filter(|git_ref| is_commit_ref(git_ref))
        .cloned()
        .collect();
    let commit_membership = membership(root, request, &tags, &branches, &pinned)?;

    let mut versions = BTreeSet::new();
    for git_ref in &request.refs {
        if let Some(version) = resolved_version(&tags, &branches, git_ref) {
            versions.insert(version);
        }
    }
    let advisories = fetch_advisories(root, request, &versions)?;

    Ok(RepoFacts {
        tags,
        branches,
        commit_membership,
        archived: Some(archived),
        advisories: Some(advisories),
        release_timestamps: BTreeMap::new(),
    })
}

fn fetch_refs(
    root: &Path,
    owner: &str,
    repo: &str,
    kind: &str,
) -> Result<BTreeMap<String, String>> {
    let endpoint = format!("repos/{owner}/{repo}/{kind}?per_page=100");
    let items = gh::api_paginated(root, &endpoint)?;
    let mut refs = BTreeMap::new();
    for item in items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .with_context(|| format!("{owner}/{repo}: {kind} entry without a name"))?;
        let sha = item
            .get("commit")
            .and_then(|commit| commit.get("sha"))
            .and_then(Value::as_str)
            .with_context(|| {
                format!("{owner}/{repo}: {kind} entry {name:?} without a commit sha")
            })?;
        refs.insert(name.to_owned(), sha.to_ascii_lowercase());
    }
    Ok(refs)
}

fn fetch_archived(root: &Path, owner: &str, repo: &str) -> Result<bool> {
    let bytes = gh::api(root, &format!("repos/{owner}/{repo}"), false)?;
    let value: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {owner}/{repo} metadata"))?;
    value
        .get("archived")
        .and_then(Value::as_bool)
        .with_context(|| {
            format!("{owner}/{repo}: repository metadata has no boolean archived flag")
        })
}

/// Resolve which refs contain each hash-pinned commit.
///
/// The audit already treats a direct tag/branch tip match as conclusive, so
/// only the remaining SHAs are probed. Containment uses GitHub's compare
/// endpoint: a `behind` or `identical` comparison against any branch or tag
/// proves the commit belongs to that ref's history; a `404` means the
/// histories are unrelated. A commit contained in no branch or tag is the
/// fork-network "impostor" case the audit reports.
fn membership(
    root: &Path,
    request: &RepoRequest,
    tags: &BTreeMap<String, String>,
    branches: &BTreeMap<String, String>,
    pinned: &BTreeSet<String>,
) -> Result<BTreeMap<String, bool>> {
    if pinned.is_empty() {
        return Ok(BTreeMap::new());
    }

    // Compare against branch tips first (usually few), then tag tips. Two
    // refs pointing at the same commit have the same history, so one is
    // enough.
    let mut bases: Vec<(String, String)> = Vec::new();
    let mut seen_tips = BTreeSet::new();
    for (name, sha) in branches {
        if seen_tips.insert(sha.clone()) {
            bases.push((format!("refs/heads/{name}"), sha.clone()));
        }
    }
    for (name, sha) in tags {
        if seen_tips.insert(sha.clone()) {
            bases.push((format!("refs/tags/{name}"), sha.clone()));
        }
    }

    let mut out = BTreeMap::new();
    for sha in pinned {
        if tags.values().any(|tip| tip == sha) || branches.values().any(|tip| tip == sha) {
            out.insert(sha.clone(), true);
            continue;
        }
        let candidates: Vec<&(String, String)> =
            bases.iter().filter(|(_, tip)| tip != sha).collect();
        let contained = contains_any(root, request, &candidates, sha)?;
        out.insert(sha.clone(), contained);
    }
    Ok(out)
}

fn contains_any(
    root: &Path,
    request: &RepoRequest,
    bases: &[&(String, String)],
    sha: &str,
) -> Result<bool> {
    // Bounded fan-out: each comparison is an independent `gh` process, and
    // repositories can have hundreds of tags. Correctness does not depend on
    // the concurrency; a found base short-circuits after its batch.
    const BATCH: usize = 8;
    for chunk in bases.chunks(BATCH) {
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|(base, _)| {
                    let owner = request.owner.clone();
                    let repo = request.repo.clone();
                    let base = base.clone();
                    let sha = sha.to_owned();
                    scope.spawn(move || compare_contains(root, &owner, &repo, &base, &sha))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("compare worker panicked"))
                .collect::<Vec<Result<bool>>>()
        });
        for result in results {
            if result? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn compare_contains(root: &Path, owner: &str, repo: &str, base: &str, sha: &str) -> Result<bool> {
    let endpoint = format!(
        "repos/{owner}/{repo}/compare/{}...{}",
        gh::encode_ref(base),
        gh::encode_ref(sha)
    );
    let Some(bytes) = gh::api_optional(root, &endpoint, false)? else {
        // No common history between the ref and the candidate commit.
        return Ok(false);
    };
    let value: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse compare response for {owner}/{repo} {base}...{sha}"))?;
    Ok(matches!(
        value.get("status").and_then(Value::as_str),
        Some("behind" | "identical")
    ))
}

/// The version string an advisory lookup should use for `git_ref`, mirroring
/// `preloop-gha-sec`'s resolution: a commit maps to its longest tag; a
/// symbolic ref maps to its branch/tag tip and then that commit's longest
/// tag, falling back to the ref itself.
fn resolved_version(
    tags: &BTreeMap<String, String>,
    branches: &BTreeMap<String, String>,
    git_ref: &str,
) -> Option<String> {
    if is_commit_ref(git_ref) {
        return longest_tag_for(tags, git_ref).map(str::to_owned);
    }
    let sha = tags.get(git_ref).or_else(|| branches.get(git_ref))?;
    Some(
        longest_tag_for(tags, sha)
            .map(str::to_owned)
            .unwrap_or_else(|| git_ref.to_owned()),
    )
}

fn longest_tag_for<'a>(tags: &'a BTreeMap<String, String>, sha: &str) -> Option<&'a str> {
    tags.iter()
        .filter(|(_, value)| value.as_str() == sha)
        .map(|(name, _)| name.as_str())
        .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
}

/// Query the global advisory database for every action version in use and
/// merge the results into one advisory list for the repository.
fn fetch_advisories(
    root: &Path,
    request: &RepoRequest,
    versions: &BTreeSet<String>,
) -> Result<Vec<AdvisoryFact>> {
    let package = format!("{}/{}", request.owner, request.repo);
    let mut advisories: BTreeMap<String, AdvisoryFact> = BTreeMap::new();

    for version in versions {
        let affects = encode_query(&format!("{package}@{version}"));
        let endpoint = format!("advisories?ecosystem=actions&affects={affects}");
        let bytes = gh::api(root, &endpoint, false)
            .with_context(|| format!("query advisories for {package}@{version}"))?;
        let items: Vec<Value> = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse advisories for {package}@{version}"))?;

        for advisory in items {
            let Some(id) = advisory.get("ghsa_id").and_then(Value::as_str) else {
                continue;
            };
            let severity = advisory_severity(
                advisory
                    .get("severity")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
            );
            let patched_version = advisory
                .get("vulnerabilities")
                .and_then(Value::as_array)
                .and_then(|vulnerabilities| {
                    let matching = vulnerabilities.iter().find(|vulnerability| {
                        let package_info = vulnerability.get("package");
                        let ecosystem = package_info
                            .and_then(|package| package.get("ecosystem"))
                            .and_then(Value::as_str);
                        let name = package_info
                            .and_then(|package| package.get("name"))
                            .and_then(Value::as_str);
                        ecosystem == Some("actions")
                            && name.is_some_and(|name| name.eq_ignore_ascii_case(&package))
                    })?;
                    matching
                        .get("first_patched_version")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            let url = advisory
                .get("html_url")
                .and_then(Value::as_str)
                .map(str::to_owned);

            let entry = advisories
                .entry(id.to_owned())
                .or_insert_with(|| AdvisoryFact {
                    id: id.to_owned(),
                    severity,
                    affected_refs: BTreeSet::new(),
                    patched_version: None,
                    url,
                });
            entry.affected_refs.insert(version.clone());
            if entry.patched_version.is_none() {
                entry.patched_version = patched_version;
            }
        }
    }

    Ok(advisories.into_values().collect())
}

fn advisory_severity(raw: &str) -> Severity {
    match raw {
        "low" => Severity::Info,
        "medium" => Severity::Warning,
        "high" | "critical" => Severity::Error,
        _ => Severity::Warning,
    }
}

/// Percent-encode a query value while leaving the characters GitHub uses in
/// advisory package coordinates (`/`, `@`) readable.
fn encode_query(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/' | b'@')
            {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn is_commit_ref(git_ref: &str) -> bool {
    git_ref.len() == 40 && git_ref.chars().all(|c| c.is_ascii_hexdigit())
}

/// Mirror `preloop-gha-sec`'s disabled-step policy so facts are gathered for
/// exactly the steps the audits will inspect.
fn is_disabled(value: &Value) -> bool {
    match value.get("if") {
        Some(Value::Bool(false)) => true,
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            let inner = trimmed
                .strip_prefix("${{")
                .and_then(|s| s.strip_suffix("}}"))
                .map(str::trim)
                .unwrap_or(trimmed);
            inner.eq_ignore_ascii_case("false")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn collect(doc: &Value) -> BTreeMap<String, RepoRequest> {
        let mut requests = BTreeMap::new();
        collect_document(doc, &mut requests);
        requests
    }

    #[test]
    fn collect_document_keeps_remote_steps_and_reusable_jobs() {
        let doc = json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        { "uses": "actions/checkout@v4" },
                        { "uses": "actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830" },
                        { "uses": "./local-action" },
                        { "uses": "docker://alpine:3.19" },
                        { "uses": "$/actions/checkout@v4" },
                        { "uses": "${{ matrix.action }}" }
                    ]
                },
                "reuse": {
                    "uses": "preloopdev/pullfrog/.github/workflows/ci.yml@v1"
                }
            }
        });
        let requests = collect(&doc);
        assert_eq!(
            requests["actions/checkout"].refs,
            BTreeSet::from(["v4".to_owned()])
        );
        assert_eq!(
            requests["actions/cache"].refs,
            BTreeSet::from(["0057852bfaa89a56745cba8c7296529d2fc39830".to_owned()])
        );
        assert_eq!(
            requests["preloopdev/pullfrog"].refs,
            BTreeSet::from(["v1".to_owned()])
        );
        assert!(!requests.contains_key("local-action"));
        assert!(!requests.contains_key("alpine"));
        assert!(!requests.contains_key("$/actions"));
    }

    #[test]
    fn collect_document_skips_disabled_jobs_and_steps() {
        let doc = json!({
            "jobs": {
                "off": {
                    "if": false,
                    "steps": [{ "uses": "actions/checkout@v4" }]
                },
                "build": {
                    "steps": [
                        { "if": false, "uses": "actions/cache@v4" },
                        { "if": "${{ false }}", "uses": "actions/setup-go@v5" }
                    ]
                }
            }
        });
        assert!(collect(&doc).is_empty());
    }

    #[test]
    fn collect_reusable_calls_keeps_only_remote_workflow_calls() {
        let doc = json!({
            "jobs": {
                "remote": {
                    "uses": "preloopdev/pullfrog/.github/workflows/ci.yml@v1"
                },
                "local": {
                    "uses": "./.github/workflows/callee.yml"
                },
                "dynamic": {
                    "uses": "${{ matrix.workflow }}"
                },
                "off": {
                    "if": false,
                    "uses": "preloopdev/pullfrog/.github/workflows/ci.yml@v1"
                },
                "action": {
                    "steps": [{ "uses": "actions/checkout@v4" }]
                }
            }
        });
        let mut calls = BTreeSet::new();
        collect_reusable_calls(&doc, &mut calls);
        assert_eq!(
            calls,
            BTreeSet::from(["preloopdev/pullfrog/.github/workflows/ci.yml@v1".to_owned()])
        );
    }

    #[test]
    fn parse_remote_call_accepts_owner_repo_path_at_ref() {
        let call =
            parse_remote_call("owner/repo/.github/workflows/ci.yml@v1").expect("remote call");
        assert_eq!(call.owner, "owner");
        assert_eq!(call.repo, "repo");
        assert_eq!(call.path, ".github/workflows/ci.yml");
        assert_eq!(call.git_ref, "v1");
        assert!(parse_remote_call("./.github/workflows/ci.yml").is_none());
        assert!(parse_remote_call("owner/repo@v1").is_none());
        assert!(parse_remote_call("owner/repo/../ci.yml@v1").is_none());
        assert!(parse_remote_call("owner/repo/.github/workflows/ci.yml@").is_none());
    }

    #[test]
    fn resolved_version_prefers_the_longest_tag() {
        let tags = BTreeMap::from([
            ("v4".to_owned(), "a".repeat(40)),
            ("v4.2.2".to_owned(), "a".repeat(40)),
            ("v5.0.0".to_owned(), "b".repeat(40)),
        ]);
        let branches = BTreeMap::from([("v4".to_owned(), "b".repeat(40))]);
        assert_eq!(
            resolved_version(&tags, &branches, &"a".repeat(40)).as_deref(),
            Some("v4.2.2")
        );
        assert_eq!(
            resolved_version(&tags, &branches, "v4").as_deref(),
            Some("v4.2.2")
        );
        assert_eq!(
            resolved_version(&tags, &branches, "missing").as_deref(),
            None
        );
    }

    #[test]
    fn advisory_severity_maps_to_the_three_level_scale() {
        assert_eq!(advisory_severity("low"), Severity::Info);
        assert_eq!(advisory_severity("medium"), Severity::Warning);
        assert_eq!(advisory_severity("high"), Severity::Error);
        assert_eq!(advisory_severity("critical"), Severity::Error);
        assert_eq!(advisory_severity("unknown"), Severity::Warning);
    }

    #[test]
    fn query_encoding_keeps_package_coordinates_readable() {
        assert_eq!(
            encode_query("actions/checkout@v4.2.2"),
            "actions/checkout@v4.2.2"
        );
        assert_eq!(encode_query("v1.0.0+build"), "v1.0.0%2Bbuild");
    }
}

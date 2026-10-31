//! Audits whose ground truth depends on repository facts that would, in a
//! live system, require querying GitHub: `impostor-commit`, `ref-confusion`,
//! `known-vulnerable-actions`, `stale-action-refs`, `ref-version-mismatch`,
//! and `archived-uses`.
//!
//! This module never makes a network call. All evidence comes from
//! [`OnlineFacts`], a snapshot the caller gathers ahead of time (by whatever
//! means it likes — a cache, a prior fetch, a fixture) and hands in through
//! [`crate::AuditContext::online`]. A `uses:` clause whose repository has no
//! entry in the snapshot is reported as [`crate::UnavailableCheck`], never
//! silently treated as clean; "unknown" and "safe" are different states
//! throughout this module.
//!
//! ## `ref-version-mismatch` scope note
//!
//! Upstream zizmor's `ref-version-mismatch` reads the inline YAML comment
//! next to a `uses: owner/repo@<sha>  # v1.2.3` line and compares the
//! claimed version against the tag the pinned commit actually resolves to.
//! This crate's audit surface is `&serde_json::Value`
//! (`WorkflowDocument::value()`), which carries no comment data, so that
//! exact mechanism isn't representable here. This crate instead flags a
//! same-document **cross-reference** mismatch: when the same action/subpath
//! is pinned by commit SHA in one place and by a symbolic ref (tag or
//! branch) in another, and the two resolve to different commits per
//! [`OnlineFacts`], that is a real, verifiable inconsistency about which
//! version of the action the workflow actually uses. It fires on a strict
//! subset of what zizmor's comment-based check would catch (both forms must
//! coexist in one document) and is not a drop-in behavioral match.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AuditContext, Confidence, Diagnostic, Severity, UnavailableCheck};

/// Where an [`OnlineFacts`] snapshot came from, for audit trails and cache
/// invalidation. Carries no capability to fetch anything itself.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DatasetProvenance {
    /// Which fact category this describes, e.g. `"refs"`, `"advisories"`,
    /// `"archived-repos"`.
    pub dataset: String,
    /// Human-readable origin, e.g. `"GitHub REST API"` or a fixture name.
    pub source: String,
    /// RFC 3339 timestamp of when the snapshot was gathered, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<String>,
}

/// A single known security advisory affecting a GitHub Action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AdvisoryFact {
    /// The advisory identifier, e.g. a GHSA id.
    pub id: String,
    /// Severity in this crate's scale; the fact provider maps the
    /// upstream advisory severity when building the snapshot.
    pub severity: Severity,
    /// Tag/version strings this advisory is known to affect, exactly as
    /// they'd be resolved from a `uses:` pin (see module docs).
    pub affected_refs: BTreeSet<String>,
    /// The first version known to fix the advisory, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patched_version: Option<String>,
    /// A reference URL for the advisory, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Explicit, caller-controlled behavior for the online rule group.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct OnlineRuleConfig {
    /// Advisory ids that are accepted despite matching
    /// `known-vulnerable-actions`, e.g. because the risk was assessed and
    /// accepted. Suppresses the finding, not the underlying advisory data.
    #[serde(default)]
    pub allowed_advisories: BTreeSet<String>,
    /// Online rule ids to skip entirely (no finding, no `unavailable`
    /// entry), e.g. `"stale-action-refs"`.
    #[serde(default)]
    pub disabled_rules: BTreeSet<String>,
}

/// Pre-fetched facts about a single `owner/repo`. Every field is
/// independently optional: a map key's presence means that category was
/// fully enumerated for this repository (even if the resulting collection
/// is empty); its absence means that category is unknown, not empty.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoFacts {
    /// Tag name to the commit SHA it points at. Presence in the owning
    /// [`OnlineFacts::repos`] entry means this map is a complete tag
    /// enumeration for the repository.
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
    /// Branch name to the commit SHA it points at. Same completeness
    /// convention as `tags`.
    #[serde(default)]
    pub branches: BTreeMap<String, String>,
    /// Explicit commit-membership results for specific SHAs, beyond what a
    /// direct `tags`/`branches` tip match already proves. `true` means the
    /// SHA was confirmed to be an ancestor of some branch or tag (not an
    /// impostor); `false` means it was confirmed absent from the
    /// repository's history (an impostor). A SHA with no entry here and no
    /// direct tip match in `tags`/`branches` is unresolved, not safe.
    #[serde(default)]
    pub commit_membership: BTreeMap<String, bool>,
    /// Whether the repository is archived, if known.
    #[serde(default)]
    pub archived: Option<bool>,
    /// Advisories known for this repository, if the advisory database was
    /// queried for it. `Some(vec![])` means it was queried and is clean;
    /// `None` means it was never queried.
    #[serde(default)]
    pub advisories: Option<Vec<AdvisoryFact>>,
    /// Commit SHA to release timestamp (RFC 3339), when known. Carried
    /// through to finding `help` text for extra context; not required by
    /// any rule's pass/fail logic.
    #[serde(default)]
    pub release_timestamps: BTreeMap<String, String>,
}

/// A caller-supplied snapshot of repository facts for the `online` rule
/// group. `None` at the [`crate::AuditContext::online`] level means no
/// snapshot was gathered at all; every online rule then reports
/// [`UnavailableCheck`] for every repository `uses:` it finds.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct OnlineFacts {
    /// Provenance of the datasets backing `repos`, for display/debugging.
    #[serde(default)]
    pub provenance: Vec<DatasetProvenance>,
    /// Rule behavior configuration.
    #[serde(default)]
    pub config: OnlineRuleConfig,
    /// Per-repository facts, keyed by lowercase `"owner/repo"`.
    #[serde(default)]
    pub repos: BTreeMap<String, RepoFacts>,
}

/// A single `uses:` clause resolved to a repository reference.
struct UsesRef<'a> {
    pointer: String,
    owner: &'a str,
    repo: &'a str,
    subpath: Option<&'a str>,
    git_ref: &'a str,
    raw: &'a str,
}

/// Run all six online rules against every repository `uses:` clause found
/// in `doc`, appending findings to `out` and unresolved checks to
/// `unavailable_out`.
pub(crate) fn check(
    doc: &Value,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
    unavailable_out: &mut Vec<UnavailableCheck>,
) {
    let refs = collect_repo_uses(doc);

    for r in &refs {
        if is_commit_ref(r.git_ref) {
            if rule_enabled(ctx, "impostor-commit") {
                impostor_commit(r, ctx, out, unavailable_out);
            }
            if rule_enabled(ctx, "stale-action-refs") {
                stale_action_refs(r, ctx, out, unavailable_out);
            }
            if rule_enabled(ctx, "ref-version-mismatch") {
                ref_version_mismatch(r, &refs, ctx, out, unavailable_out);
            }
        } else if rule_enabled(ctx, "ref-confusion") {
            ref_confusion(r, ctx, out, unavailable_out);
        }

        if rule_enabled(ctx, "archived-uses") {
            archived_uses(r, ctx, out, unavailable_out);
        }
        if rule_enabled(ctx, "known-vulnerable-actions") {
            known_vulnerable_actions(r, ctx, out, unavailable_out);
        }
    }
}

fn rule_enabled(ctx: &AuditContext, rule_id: &str) -> bool {
    !ctx.online
        .as_ref()
        .is_some_and(|o| o.config.disabled_rules.contains(rule_id))
}

fn repo_key(owner: &str, repo: &str) -> String {
    format!("{}/{}", owner.to_lowercase(), repo.to_lowercase())
}

fn lookup<'a>(ctx: &'a AuditContext, owner: &str, repo: &str) -> Option<&'a RepoFacts> {
    ctx.online.as_ref()?.repos.get(&repo_key(owner, repo))
}

fn unavailable(rule_id: &str, pointer: &str, reason: impl Into<String>) -> UnavailableCheck {
    UnavailableCheck {
        rule_id: rule_id.to_owned(),
        path: pointer.to_owned(),
        reason: reason.into(),
    }
}

/// A Git object id: exactly 40 hex digits, matching GitHub's full-length
/// commit SHAs (and, incidentally, annotated tag object ids).
fn is_commit_ref(git_ref: &str) -> bool {
    git_ref.len() == 40 && git_ref.chars().all(|c| c.is_ascii_hexdigit())
}

/// Parse a `uses:` string into `(owner, repo, subpath, git_ref)`. Returns
/// `None` for local actions (`./path`), Docker references
/// (`docker://image`), or anything else that isn't a pinned repository
/// reference.
fn parse_repo_uses(raw: &str) -> Option<(&str, &str, Option<&str>, &str)> {
    if raw.starts_with("./") || raw.starts_with("../") || raw.starts_with("docker://") {
        return None;
    }
    let (path, git_ref) = raw.rsplit_once('@')?;
    if path.is_empty() || git_ref.is_empty() {
        return None;
    }
    let mut parts = path.splitn(3, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    let subpath = parts.next().filter(|s| !s.is_empty());
    Some((owner, repo, subpath, git_ref))
}

/// Only a literal `false` condition disables an online check for a job or
/// step; this mirrors `preloop_gha_lint::is_disabled`'s policy exactly
/// (duplicated locally per this crate's module-isolation contract) so a
/// disabled step's `uses:` doesn't generate supply-chain findings for code
/// that can never execute.
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

fn collect_repo_uses(doc: &Value) -> Vec<UsesRef<'_>> {
    let mut out = Vec::new();
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return out;
    };

    for (job_id, job) in jobs {
        if is_disabled(job) {
            continue;
        }
        let job_pointer = crate::pointer("/jobs", job_id);

        if let Some(Value::String(uses)) = job.get("uses") {
            push_uses(&mut out, crate::pointer(&job_pointer, "uses"), uses);
        }

        if let Some(steps) = job.get("steps").and_then(Value::as_array) {
            let steps_pointer = crate::pointer(&job_pointer, "steps");
            for (idx, step) in steps.iter().enumerate() {
                if is_disabled(step) {
                    continue;
                }
                if let Some(Value::String(uses)) = step.get("uses") {
                    let step_pointer = crate::pointer(&steps_pointer, &idx.to_string());
                    push_uses(&mut out, crate::pointer(&step_pointer, "uses"), uses);
                }
            }
        }
    }

    out
}

fn push_uses<'a>(out: &mut Vec<UsesRef<'a>>, pointer: String, raw: &'a str) {
    if let Some((owner, repo, subpath, git_ref)) = parse_repo_uses(raw) {
        out.push(UsesRef {
            pointer,
            owner,
            repo,
            subpath,
            git_ref,
            raw,
        });
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// Longest tag name pointing at `sha`, matching zizmor's heuristic for
/// normalizing a version branch (e.g. `v1`) to its most specific tag (e.g.
/// `v1.2.3`). Ties break alphabetically for determinism.
fn longest_tag_for<'a>(facts: &'a RepoFacts, sha: &str) -> Option<&'a str> {
    facts
        .tags
        .iter()
        .filter(|(_, v)| v.as_str() == sha)
        .map(|(k, _)| k.as_str())
        .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
}

// --- impostor-commit --------------------------------------------------

fn impostor_commit(
    r: &UsesRef,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
    unavail: &mut Vec<UnavailableCheck>,
) {
    const RULE: &str = "impostor-commit";

    let Some(facts) = lookup(ctx, r.owner, r.repo) else {
        unavail.push(unavailable(
            RULE,
            &r.pointer,
            format!("no repository evidence for {}/{}", r.owner, r.repo),
        ));
        return;
    };

    if let Some(belongs) = facts.commit_membership.get(r.git_ref) {
        if !*belongs {
            let mut d = crate::finding(
                RULE,
                &r.pointer,
                format!(
                    "`{}` pins a commit that does not belong to {}/{}'s history",
                    r.raw, r.owner, r.repo
                ),
                "This looks like an impostor commit reachable only through GitHub's fork \
                 network, not an ancestor of any branch or tag in the named repository. \
                 Verify the SHA against the upstream repository before trusting it.",
            );
            d.severity = Severity::Error;
            out.push(d);
        }
        return;
    }

    // Fastest-path equivalent: a direct tip match on a known tag or branch
    // is conclusive proof of non-impostor status even without an explicit
    // `commit_membership` entry.
    if facts.tags.values().any(|sha| sha == r.git_ref)
        || facts.branches.values().any(|sha| sha == r.git_ref)
    {
        return;
    }

    unavail.push(unavailable(
        RULE,
        &r.pointer,
        format!(
            "commit {} is not a known tag/branch tip of {}/{} and its repository-history \
             membership was not resolved",
            short(r.git_ref),
            r.owner,
            r.repo
        ),
    ));
}

// --- ref-confusion ------------------------------------------------------

fn ref_confusion(
    r: &UsesRef,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
    unavail: &mut Vec<UnavailableCheck>,
) {
    const RULE: &str = "ref-confusion";

    let Some(facts) = lookup(ctx, r.owner, r.repo) else {
        unavail.push(unavailable(
            RULE,
            &r.pointer,
            format!("no branch/tag evidence for {}/{}", r.owner, r.repo),
        ));
        return;
    };

    if facts.tags.contains_key(r.git_ref) && facts.branches.contains_key(r.git_ref) {
        let mut d = crate::finding(
            RULE,
            &r.pointer,
            format!(
                "`{}` is ambiguous: {}/{} has both a branch and a tag named `{}`",
                r.raw, r.owner, r.repo, r.git_ref
            ),
            "Pin to a commit SHA, or qualify the ref with refs/heads/ or refs/tags/ to remove \
             the ambiguity about which namespace is selected.",
        );
        d.severity = Severity::Warning;
        d.confidence = Confidence::High;
        out.push(d);
    }
}

// --- stale-action-refs ---------------------------------------------------

fn stale_action_refs(
    r: &UsesRef,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
    unavail: &mut Vec<UnavailableCheck>,
) {
    const RULE: &str = "stale-action-refs";

    // Upstream zizmor gates this behind its "pedantic" persona (code smell,
    // not a problem). Below that persona the rule simply doesn't apply;
    // that's a policy choice, not missing evidence, so no `unavailable`.
    if ctx.persona == crate::Persona::Regular {
        return;
    }

    let Some(facts) = lookup(ctx, r.owner, r.repo) else {
        unavail.push(unavailable(
            RULE,
            &r.pointer,
            format!("no tag evidence for {}/{}", r.owner, r.repo),
        ));
        return;
    };

    let has_tag = facts.tags.values().any(|sha| sha == r.git_ref);
    if !has_tag {
        let mut d = crate::finding(
            RULE,
            &r.pointer,
            format!(
                "`{}` pins a commit hash with no corresponding tag in {}/{}",
                r.raw, r.owner, r.repo
            ),
            "A hash pin with no matching tag makes it hard to tell which release introduced \
             the commit; prefer pinning the commit behind a tagged release.",
        );
        d.severity = Severity::Info;
        out.push(d);
    }
}

// --- ref-version-mismatch -------------------------------------------------

fn ref_version_mismatch(
    r: &UsesRef,
    all: &[UsesRef],
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
    unavail: &mut Vec<UnavailableCheck>,
) {
    const RULE: &str = "ref-version-mismatch";

    // Find another `uses:` of the same action/subpath elsewhere in the
    // document that's pinned by a symbolic ref instead of a commit. See the
    // module-level doc comment for why this is the adapted form of the
    // upstream comment-based check.
    let Some(companion) = all.iter().find(|c| {
        c.pointer != r.pointer
            && c.owner.eq_ignore_ascii_case(r.owner)
            && c.repo.eq_ignore_ascii_case(r.repo)
            && c.subpath == r.subpath
            && !is_commit_ref(c.git_ref)
    }) else {
        return;
    };

    let Some(facts) = lookup(ctx, r.owner, r.repo) else {
        unavail.push(unavailable(
            RULE,
            &r.pointer,
            format!(
                "no branch/tag evidence to resolve `{}` ({}) for {}/{}",
                companion.git_ref, companion.pointer, r.owner, r.repo
            ),
        ));
        return;
    };

    let resolved_sha = facts
        .tags
        .get(companion.git_ref)
        .or_else(|| facts.branches.get(companion.git_ref));

    let Some(resolved_sha) = resolved_sha else {
        unavail.push(unavailable(
            RULE,
            &r.pointer,
            format!(
                "`{}` ({}) does not resolve to a known tag or branch in {}/{}",
                companion.git_ref, companion.pointer, r.owner, r.repo
            ),
        ));
        return;
    };

    if resolved_sha != r.git_ref {
        let mut d = crate::finding(
            RULE,
            &r.pointer,
            format!(
                "pins `{}` to {}, but `{}` ({}) resolves to a different commit ({}) for the \
                 same action in this document",
                r.raw,
                short(r.git_ref),
                companion.raw,
                companion.pointer,
                short(resolved_sha)
            ),
            "These two references to the same action disagree about which commit is current; \
             repin both to the same, verified commit.",
        );
        d.severity = Severity::Warning;
        out.push(d);
    }
}

// --- known-vulnerable-actions ----------------------------------------------

fn known_vulnerable_actions(
    r: &UsesRef,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
    unavail: &mut Vec<UnavailableCheck>,
) {
    const RULE: &str = "known-vulnerable-actions";

    let Some(facts) = lookup(ctx, r.owner, r.repo) else {
        unavail.push(unavailable(
            RULE,
            &r.pointer,
            format!("no repository evidence for {}/{}", r.owner, r.repo),
        ));
        return;
    };

    let resolved_version: String = if is_commit_ref(r.git_ref) {
        match longest_tag_for(facts, r.git_ref) {
            Some(tag) => tag.to_owned(),
            // No tag corresponds to this commit; there's no version to look
            // up advisories against. Not an error, just nothing to check
            // (matches zizmor's own fallthrough for this case).
            None => return,
        }
    } else {
        let resolved_sha = facts
            .tags
            .get(r.git_ref)
            .or_else(|| facts.branches.get(r.git_ref));
        match resolved_sha {
            // The ref doesn't exist per a complete tag/branch enumeration;
            // nothing to check.
            None => return,
            Some(sha) => longest_tag_for(facts, sha)
                .map(str::to_owned)
                .unwrap_or_else(|| r.git_ref.to_owned()),
        }
    };

    let Some(advisories) = &facts.advisories else {
        unavail.push(unavailable(
            RULE,
            &r.pointer,
            format!(
                "advisory database was not queried for {}/{}",
                r.owner, r.repo
            ),
        ));
        return;
    };

    let allowed = ctx
        .online
        .as_ref()
        .map(|o| &o.config.allowed_advisories)
        .cloned()
        .unwrap_or_default();

    for advisory in advisories {
        if allowed.contains(&advisory.id) {
            continue;
        }
        if !advisory.affected_refs.contains(&resolved_version) {
            continue;
        }

        let mut d = crate::finding(
            RULE,
            &r.pointer,
            format!(
                "`{}` resolves to version `{}`, which is affected by {}",
                r.raw, resolved_version, advisory.id
            ),
            "Upgrade to a patched version.",
        );
        d.severity = advisory.severity;
        d.confidence = Confidence::High;
        d.help = Some(match &advisory.patched_version {
            Some(patched) => format!(
                "Upgrade to {patched} or later, which fixes {}.",
                advisory.id
            ),
            None => format!(
                "No patched version is recorded yet for {}; pin to a commit predating the \
                 affected release or remove the dependency until one is available.",
                advisory.id
            ),
        });
        out.push(d);
    }
}

// --- archived-uses ---------------------------------------------------------

fn archived_uses(
    r: &UsesRef,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
    unavail: &mut Vec<UnavailableCheck>,
) {
    const RULE: &str = "archived-uses";

    let Some(facts) = lookup(ctx, r.owner, r.repo) else {
        unavail.push(unavailable(
            RULE,
            &r.pointer,
            format!("no archived-repository evidence for {}/{}", r.owner, r.repo),
        ));
        return;
    };

    match facts.archived {
        None => unavail.push(unavailable(
            RULE,
            &r.pointer,
            format!("archived status of {}/{} was not recorded", r.owner, r.repo),
        )),
        Some(false) => {}
        Some(true) => {
            let mut d = crate::finding(
                RULE,
                &r.pointer,
                format!(
                    "`{}` is hosted in an archived repository ({}/{})",
                    r.raw, r.owner, r.repo
                ),
                "Archived repositories receive no further updates, including security fixes; \
                 fork or vendor the action instead of depending on it as-is.",
            );
            d.severity = Severity::Warning;
            out.push(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Persona;

    fn ctx_with(facts: OnlineFacts) -> AuditContext {
        AuditContext {
            online: Some(facts),
            ..Default::default()
        }
    }

    fn run(doc: &Value, ctx: &AuditContext) -> (Vec<Diagnostic>, Vec<UnavailableCheck>) {
        let mut out = Vec::new();
        let mut unavail = Vec::new();
        check(doc, ctx, &mut out, &mut unavail);
        (out, unavail)
    }

    const SHA_GOOD: &str = "1111111111111111111111111111111111111111";
    const SHA_IMPOSTOR: &str = "2222222222222222222222222222222222222222";

    fn checkout_doc(uses: &str) -> Value {
        json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        { "uses": uses }
                    ]
                }
            }
        })
    }

    // -- impostor-commit --

    #[test]
    fn impostor_commit_safe_when_sha_matches_tag_tip() {
        let doc = checkout_doc(&format!("actions/checkout@{SHA_GOOD}"));
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                tags: BTreeMap::from([("v4".to_owned(), SHA_GOOD.to_owned())]),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, unavail) = run(&doc, &ctx);
        assert!(
            !findings.iter().any(|d| d.rule_id == "impostor-commit"),
            "{findings:?}"
        );
        assert!(
            !unavail.iter().any(|u| u.rule_id == "impostor-commit"),
            "{unavail:?}"
        );
    }

    #[test]
    fn impostor_commit_unsafe_when_membership_confirmed_absent() {
        let doc = checkout_doc(&format!("actions/checkout@{SHA_IMPOSTOR}"));
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                commit_membership: BTreeMap::from([(SHA_IMPOSTOR.to_owned(), false)]),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, _) = run(&doc, &ctx);
        let f = findings
            .iter()
            .find(|d| d.rule_id == "impostor-commit")
            .expect("expected impostor-commit finding");
        assert_eq!(f.severity, Severity::Error);
        assert_eq!(f.source, "preloop-gha-sec");
    }

    #[test]
    fn impostor_commit_unavailable_without_repo_facts() {
        let doc = checkout_doc(&format!("actions/checkout@{SHA_IMPOSTOR}"));
        let ctx = ctx_with(OnlineFacts::default());
        let (findings, unavail) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "impostor-commit"));
        assert!(unavail.iter().any(|u| u.rule_id == "impostor-commit"));
    }

    #[test]
    fn impostor_commit_unavailable_when_unresolved_and_no_tip_match() {
        let doc = checkout_doc(&format!("actions/checkout@{SHA_IMPOSTOR}"));
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                tags: BTreeMap::from([("v4".to_owned(), SHA_GOOD.to_owned())]),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, unavail) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "impostor-commit"));
        assert!(unavail.iter().any(|u| u.rule_id == "impostor-commit"));
    }

    // -- ref-confusion --

    #[test]
    fn ref_confusion_fires_on_branch_and_tag_collision() {
        let doc = checkout_doc("actions/checkout@v1");
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                tags: BTreeMap::from([("v1".to_owned(), SHA_GOOD.to_owned())]),
                branches: BTreeMap::from([("v1".to_owned(), SHA_IMPOSTOR.to_owned())]),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, _) = run(&doc, &ctx);
        let f = findings
            .iter()
            .find(|d| d.rule_id == "ref-confusion")
            .expect("expected ref-confusion finding");
        assert_eq!(f.severity, Severity::Warning);
    }

    #[test]
    fn ref_confusion_safe_when_only_tag_exists() {
        let doc = checkout_doc("actions/checkout@v1");
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                tags: BTreeMap::from([("v1".to_owned(), SHA_GOOD.to_owned())]),
                branches: BTreeMap::new(),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, unavail) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "ref-confusion"));
        assert!(!unavail.iter().any(|u| u.rule_id == "ref-confusion"));
    }

    #[test]
    fn ref_confusion_unavailable_without_repo_facts() {
        let doc = checkout_doc("actions/checkout@v1");
        let ctx = ctx_with(OnlineFacts::default());
        let (_, unavail) = run(&doc, &ctx);
        assert!(unavail.iter().any(|u| u.rule_id == "ref-confusion"));
    }

    // -- stale-action-refs --

    #[test]
    fn stale_action_refs_fires_under_pedantic_persona_when_no_tag_matches() {
        let doc = checkout_doc(&format!("actions/checkout@{SHA_GOOD}"));
        let mut repos = BTreeMap::new();
        repos.insert("actions/checkout".to_owned(), RepoFacts::default());
        let ctx = AuditContext {
            online: Some(OnlineFacts {
                repos,
                ..Default::default()
            }),
            persona: Persona::Pedantic,
            ..Default::default()
        };
        let (findings, _) = run(&doc, &ctx);
        let f = findings
            .iter()
            .find(|d| d.rule_id == "stale-action-refs")
            .expect("expected stale-action-refs finding");
        assert_eq!(f.severity, Severity::Info);
    }

    #[test]
    fn stale_action_refs_silent_under_regular_persona() {
        let doc = checkout_doc(&format!("actions/checkout@{SHA_GOOD}"));
        let mut repos = BTreeMap::new();
        repos.insert("actions/checkout".to_owned(), RepoFacts::default());
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        }); // default persona = Regular
        let (findings, unavail) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "stale-action-refs"));
        assert!(!unavail.iter().any(|u| u.rule_id == "stale-action-refs"));
    }

    #[test]
    fn stale_action_refs_safe_when_tag_matches() {
        let doc = checkout_doc(&format!("actions/checkout@{SHA_GOOD}"));
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                tags: BTreeMap::from([("v4.2.0".to_owned(), SHA_GOOD.to_owned())]),
                ..Default::default()
            },
        );
        let ctx = AuditContext {
            online: Some(OnlineFacts {
                repos,
                ..Default::default()
            }),
            persona: Persona::Pedantic,
            ..Default::default()
        };
        let (findings, _) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "stale-action-refs"));
    }

    // -- ref-version-mismatch --

    #[test]
    fn ref_version_mismatch_fires_on_cross_reference_disagreement() {
        let doc = json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        { "uses": format!("actions/checkout@{SHA_IMPOSTOR}") },
                        { "uses": "actions/checkout@v4" }
                    ]
                }
            }
        });
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                tags: BTreeMap::from([("v4".to_owned(), SHA_GOOD.to_owned())]),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, _) = run(&doc, &ctx);
        let f = findings
            .iter()
            .find(|d| d.rule_id == "ref-version-mismatch")
            .expect("expected ref-version-mismatch finding");
        assert_eq!(f.path, "/jobs/build/steps/0/uses");
        assert_eq!(f.severity, Severity::Warning);
    }

    #[test]
    fn ref_version_mismatch_safe_when_cross_reference_agrees() {
        let doc = json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        { "uses": format!("actions/checkout@{SHA_GOOD}") },
                        { "uses": "actions/checkout@v4" }
                    ]
                }
            }
        });
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                tags: BTreeMap::from([("v4".to_owned(), SHA_GOOD.to_owned())]),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, unavail) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "ref-version-mismatch"));
        assert!(!unavail.iter().any(|u| u.rule_id == "ref-version-mismatch"));
    }

    #[test]
    fn ref_version_mismatch_not_applicable_without_companion_ref() {
        let doc = checkout_doc(&format!("actions/checkout@{SHA_GOOD}"));
        let ctx = ctx_with(OnlineFacts::default());
        let (findings, unavail) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "ref-version-mismatch"));
        assert!(!unavail.iter().any(|u| u.rule_id == "ref-version-mismatch"));
    }

    #[test]
    fn ref_version_mismatch_unavailable_when_companion_ref_unresolved() {
        let doc = json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        { "uses": format!("actions/checkout@{SHA_GOOD}") },
                        { "uses": "actions/checkout@v4" }
                    ]
                }
            }
        });
        let ctx = ctx_with(OnlineFacts::default());
        let (findings, unavail) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "ref-version-mismatch"));
        assert!(unavail.iter().any(|u| u.rule_id == "ref-version-mismatch"));
    }

    // -- known-vulnerable-actions --

    #[test]
    fn known_vulnerable_actions_fires_for_affected_tag() {
        let doc = checkout_doc("actions/checkout@v2");
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                tags: BTreeMap::from([("v2".to_owned(), SHA_GOOD.to_owned())]),
                advisories: Some(vec![AdvisoryFact {
                    id: "GHSA-aaaa-bbbb-cccc".to_owned(),
                    severity: Severity::Error,
                    affected_refs: BTreeSet::from(["v2".to_owned()]),
                    patched_version: Some("v3".to_owned()),
                    url: None,
                }]),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, _) = run(&doc, &ctx);
        let f = findings
            .iter()
            .find(|d| d.rule_id == "known-vulnerable-actions")
            .expect("expected known-vulnerable-actions finding");
        assert_eq!(f.severity, Severity::Error);
        assert!(f.help.as_ref().unwrap().contains("v3"));
    }

    #[test]
    fn known_vulnerable_actions_safe_for_patched_tag() {
        let doc = checkout_doc("actions/checkout@v3");
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                tags: BTreeMap::from([("v3".to_owned(), SHA_IMPOSTOR.to_owned())]),
                advisories: Some(vec![AdvisoryFact {
                    id: "GHSA-aaaa-bbbb-cccc".to_owned(),
                    severity: Severity::Error,
                    affected_refs: BTreeSet::from(["v2".to_owned()]),
                    patched_version: Some("v3".to_owned()),
                    url: None,
                }]),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, unavail) = run(&doc, &ctx);
        assert!(
            !findings
                .iter()
                .any(|d| d.rule_id == "known-vulnerable-actions")
        );
        assert!(
            !unavail
                .iter()
                .any(|u| u.rule_id == "known-vulnerable-actions")
        );
    }

    #[test]
    fn known_vulnerable_actions_allowed_advisory_suppressed() {
        let doc = checkout_doc("actions/checkout@v2");
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                tags: BTreeMap::from([("v2".to_owned(), SHA_GOOD.to_owned())]),
                advisories: Some(vec![AdvisoryFact {
                    id: "GHSA-aaaa-bbbb-cccc".to_owned(),
                    severity: Severity::Error,
                    affected_refs: BTreeSet::from(["v2".to_owned()]),
                    patched_version: None,
                    url: None,
                }]),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            config: OnlineRuleConfig {
                allowed_advisories: BTreeSet::from(["GHSA-aaaa-bbbb-cccc".to_owned()]),
                ..Default::default()
            },
            ..Default::default()
        });
        let (findings, _) = run(&doc, &ctx);
        assert!(
            !findings
                .iter()
                .any(|d| d.rule_id == "known-vulnerable-actions")
        );
    }

    #[test]
    fn known_vulnerable_actions_unavailable_without_advisory_query() {
        let doc = checkout_doc("actions/checkout@v2");
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                tags: BTreeMap::from([("v2".to_owned(), SHA_GOOD.to_owned())]),
                advisories: None,
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, unavail) = run(&doc, &ctx);
        assert!(
            !findings
                .iter()
                .any(|d| d.rule_id == "known-vulnerable-actions")
        );
        assert!(
            unavail
                .iter()
                .any(|u| u.rule_id == "known-vulnerable-actions")
        );
    }

    // -- archived-uses --

    #[test]
    fn archived_uses_fires_when_confirmed_archived() {
        let doc = checkout_doc("actions/checkout@v4");
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                archived: Some(true),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, _) = run(&doc, &ctx);
        let f = findings
            .iter()
            .find(|d| d.rule_id == "archived-uses")
            .expect("expected archived-uses finding");
        assert_eq!(f.severity, Severity::Warning);
    }

    #[test]
    fn archived_uses_safe_when_confirmed_not_archived() {
        let doc = checkout_doc("actions/checkout@v4");
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                archived: Some(false),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, unavail) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "archived-uses"));
        assert!(!unavail.iter().any(|u| u.rule_id == "archived-uses"));
    }

    #[test]
    fn archived_uses_unavailable_when_status_unknown() {
        let doc = checkout_doc("actions/checkout@v4");
        let mut repos = BTreeMap::new();
        repos.insert("actions/checkout".to_owned(), RepoFacts::default());
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, unavail) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "archived-uses"));
        assert!(unavail.iter().any(|u| u.rule_id == "archived-uses"));
    }

    #[test]
    fn archived_uses_unavailable_without_online_facts_at_all() {
        let doc = checkout_doc("actions/checkout@v4");
        let ctx = AuditContext::default();
        let (findings, unavail) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "archived-uses"));
        assert!(unavail.iter().any(|u| u.rule_id == "archived-uses"));
    }

    // -- disabled rules / if:false --

    #[test]
    fn disabled_step_is_not_audited() {
        let doc = json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        { "if": false, "uses": format!("actions/checkout@{SHA_IMPOSTOR}") }
                    ]
                }
            }
        });
        let mut repos = BTreeMap::new();
        repos.insert(
            "actions/checkout".to_owned(),
            RepoFacts {
                commit_membership: BTreeMap::from([(SHA_IMPOSTOR.to_owned(), false)]),
                archived: Some(true),
                ..Default::default()
            },
        );
        let ctx = ctx_with(OnlineFacts {
            repos,
            ..Default::default()
        });
        let (findings, unavail) = run(&doc, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
        assert!(unavail.is_empty(), "{unavail:?}");
    }

    #[test]
    fn disabled_rule_config_suppresses_rule_entirely() {
        let doc = checkout_doc("actions/checkout@v4");
        let mut repos = BTreeMap::new();
        repos.insert("actions/checkout".to_owned(), RepoFacts::default());
        let ctx = ctx_with(OnlineFacts {
            repos,
            config: OnlineRuleConfig {
                disabled_rules: BTreeSet::from(["archived-uses".to_owned()]),
                ..Default::default()
            },
            ..Default::default()
        });
        let (findings, unavail) = run(&doc, &ctx);
        assert!(!findings.iter().any(|d| d.rule_id == "archived-uses"));
        assert!(!unavail.iter().any(|u| u.rule_id == "archived-uses"));
    }

    #[test]
    fn local_and_docker_uses_are_ignored() {
        let doc = json!({
            "jobs": {
                "build": {
                    "runs-on": "ubuntu-latest",
                    "steps": [
                        { "uses": "./local-action" },
                        { "uses": "docker://alpine:3.19" }
                    ]
                }
            }
        });
        let ctx = ctx_with(OnlineFacts::default());
        let (findings, unavail) = run(&doc, &ctx);
        assert!(findings.is_empty());
        assert!(unavail.is_empty());
    }
}

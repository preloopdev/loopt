//! Offline security and correctness audits for GitHub Actions authoring
//! documents.
//!
//! This crate re-implements, as an independently-licensed MIT codebase, the
//! set of audits registered by [zizmor] (Apache-2.0) plus a small number of
//! higher-confidence additions, combined with correctness findings relevant
//! to Preloop and Loopt's execution model. It never starts a subprocess,
//! opens a socket, or reads credentials; any evidence that would otherwise
//! require a live network call (resolved commit history, archived-repo
//! status, published advisories, and similar) is instead supplied by the
//! caller as a pre-fetched [`OnlineFacts`] snapshot through [`AuditContext`].
//! Absent evidence is always reported as [`UnavailableCheck`], never
//! silently treated as "safe".
//!
//! [zizmor]: https://github.com/woodruffw/zizmor
//!
//! # Rule ownership
//!
//! - `trust` — trust-boundary audits (e.g. `dangerous-triggers`,
//!   `bot-conditions`, `template-injection`, `cache-poisoning`, `github-env`).
//! - `secrets` — secret-handling audits (e.g. `secrets-inherit`,
//!   `unredacted-secrets`, `overprovisioned-secrets`).
//! - `supply` — supply-chain/pinning audits (e.g. `unpinned-uses`,
//!   `typosquat-uses`, `forbidden-uses`, `obfuscation`).
//! - `policy` — permissions/configuration audits (e.g.
//!   `excessive-permissions`, `self-hosted-runner`, `insecure-commands`).
//! - `online` — audits whose ground truth depends on pre-fetched repository
//!   facts: `impostor-commit`, `ref-confusion`, `known-vulnerable-actions`,
//!   `stale-action-refs`, `ref-version-mismatch`, `archived-uses`.

use std::collections::BTreeSet;

use preloop_gha_authoring::WorkflowDocument;
use serde::{Deserialize, Serialize};

pub use preloop_gha_lint::{Confidence, Diagnostic, Severity};

mod online;
mod policy;
mod secrets;
mod supply;
mod trust;

pub use online::{AdvisoryFact, DatasetProvenance, OnlineFacts, OnlineRuleConfig, RepoFacts};

/// The reviewer persona a caller wants findings tuned for, mirroring
/// zizmor's three-tier model. Individual rules consult this to decide
/// whether a borderline observation is worth surfacing.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Persona {
    /// Minimal false positives: only report high-confidence problems.
    #[default]
    Regular,
    /// Code smells are acceptable: report findings that may or may not be
    /// problems but are worth a human glance.
    Pedantic,
    /// All results are acceptable, including likely false positives.
    Auditor,
}

/// Caller-supplied evidence and policy for a single audit run. Every field
/// is independently optional/empty-meaning-unknown; the audit never infers
/// missing context from the document itself.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditContext {
    /// Pre-fetched repository facts for `online`-owned rules. `None` means
    /// no facts were gathered at all; every online rule reports
    /// [`UnavailableCheck`] for every `uses:` it cannot resolve.
    #[serde(default)]
    pub online: Option<OnlineFacts>,
    /// `owner/repo` (or `owner/repo/path`) strings that are never acceptable
    /// in a `uses:` clause, regardless of pin, compared case-insensitively.
    #[serde(default)]
    pub forbidden_uses: Vec<String>,
    /// `owner/repo` strings explicitly allow-listed by policy, silencing
    /// supply-chain advisories that would otherwise fire for them.
    #[serde(default)]
    pub allowed_uses: Vec<String>,
    /// Persona controlling which borderline findings are emitted.
    #[serde(default)]
    pub persona: Persona,
    /// Workflow-wide secret name inventory; `None` means unknown. Mirrors
    /// `preloop_gha_lint::LintContext::available_secrets` but is supplied
    /// independently since this crate never depends on the lint crate's
    /// context type.
    #[serde(default)]
    pub known_secret_names: Option<BTreeSet<String>>,
}

/// A rule that could not be evaluated for a specific location because the
/// evidence it depends on was absent or incomplete. Distinct from a clean
/// finding: unknown is never reported as safe.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnavailableCheck {
    /// The rule that could not run, e.g. `"impostor-commit"`.
    pub rule_id: String,
    /// RFC 6901 JSON Pointer to the location that needed evidence.
    pub path: String,
    /// Human-readable explanation of what evidence was missing.
    pub reason: String,
}

/// The result of auditing one document: findings plus a record of every
/// check that could not reach a conclusion.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditReport {
    /// Findings ordered by JSON Pointer, then rule, then source, then
    /// message.
    pub findings: Vec<Diagnostic>,
    /// Checks that were skipped for lack of evidence, in rule-group
    /// evaluation order.
    pub unavailable: Vec<UnavailableCheck>,
}

/// Run every registered audit against `document` using `context` for
/// policy and (optionally) pre-fetched online evidence.
///
/// Findings are deterministically ordered. Absent online evidence produces
/// `unavailable` entries rather than being silently dropped; a caller that
/// ignores `unavailable` is choosing to treat unknown as unknown, not safe.
pub fn audit(document: &WorkflowDocument, context: &AuditContext) -> AuditReport {
    let doc = document.value();

    let mut findings = Vec::new();
    let mut unavailable = Vec::new();

    trust::check(doc, context, &mut findings);
    secrets::check(doc, context, &mut findings);
    supply::check(doc, context, &mut findings);
    policy::check(doc, document.source(), context, &mut findings);
    online::check(doc, context, &mut findings, &mut unavailable);

    findings.sort_by(|a, b| {
        (&a.path, &a.rule_id, a.source.as_str(), &a.message).cmp(&(
            &b.path,
            &b.rule_id,
            b.source.as_str(),
            &b.message,
        ))
    });
    unavailable
        .sort_by(|a, b| (&a.path, &a.rule_id, &a.reason).cmp(&(&b.path, &b.rule_id, &b.reason)));

    AuditReport {
        findings,
        unavailable,
    }
}

/// Build a [`Diagnostic`] with this crate's defaults: `Severity::Warning`,
/// `Confidence::High`, `source: "preloop-gha-sec"`, no source coordinates.
/// Rule groups override whichever fields their finding actually needs.
pub(crate) fn finding(id: &str, path: &str, msg: impl Into<String>, help: &str) -> Diagnostic {
    Diagnostic {
        rule_id: id.to_owned(),
        severity: Severity::Warning,
        confidence: Confidence::High,
        path: path.to_owned(),
        message: msg.into(),
        help: Some(help.to_owned()),
        line: None,
        column: None,
        source: "preloop-gha-sec".to_owned(),
    }
}

/// Append `key` to `parent` as an RFC 6901 JSON Pointer segment, escaping
/// `~` and `/` per the spec (`~0`, `~1`).
pub(crate) fn pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_escapes_tilde_and_slash() {
        assert_eq!(pointer("/jobs", "a/b~c"), "/jobs/a~1b~0c");
    }

    #[test]
    fn finding_defaults_are_sec_source() {
        let d = finding("x", "/p", "msg", "help");
        assert_eq!(d.source, "preloop-gha-sec");
        assert_eq!(d.severity, Severity::Warning);
        assert_eq!(d.confidence, Confidence::High);
    }

    #[test]
    fn persona_defaults_to_regular() {
        assert_eq!(Persona::default(), Persona::Regular);
    }
}

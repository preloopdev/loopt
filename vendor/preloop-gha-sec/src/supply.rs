//! Native reimplementations of nine zizmor (MIT) supply-chain audits:
//! `forbidden-uses`, `obfuscation`, `unpinned-uses`, `unpinned-images`,
//! `typosquat-uses`, `unpinned-tools`, `adhoc-packages`,
//! `insecure-url-scheme`, and `self-repository`.
//!
//! Scope and attribution: the structural logic below was studied from the
//! MIT-licensed zizmor audits at
//! `crates/zizmor/src/audit/{forbidden_uses,obfuscation,unpinned_uses,
//! unpinned_images,typosquat_uses,unpinned_tools,adhoc_packages,
//! insecure_url_scheme,self_repository}.rs` in
//! <https://github.com/zizmorcore/zizmor> (MIT licensed; see that project's
//! `LICENSE`). No zizmor code is linked into or invoked by this crate; this
//! is an independent, offline reimplementation against `serde_json::Value`
//! rather than zizmor's typed `github_actions_models`/tree-sitter pipeline,
//! and intentionally narrows several checks where zizmor relies on inputs or
//! dependencies we don't have (documented per-function below):
//!
//! - No `tree-sitter` bash/pwsh grammars: `adhoc-packages` and
//!   `insecure-url-scheme` use a hand-rolled simple-command tokenizer
//!   (quote-aware, operator-splitting) instead of a real shell AST, and only
//!   understand `bash`/`sh`/`zsh`-style scripts; `pwsh`/`powershell` and any
//!   other `shell:` are left unanalyzed (no finding, not "safe").
//! - No `typomania` crate and no live GitHub lookups: `typosquat-uses` uses a
//!   small, independently-compiled sample of well-known action slugs (not
//!   zizmor's `data/popular-actions.txt`) and a native optimal-string-
//!   alignment distance check instead of zizmor's keyboard-adjacency/omitted-
//!   character/swapped-word heuristics; confidence is capped at `Medium`
//!   because we can't confirm the candidate repository actually exists.
//! - `insecure-url-scheme` in zizmor only inspects pre-commit `repo:` URLs
//!   (we don't support pre-commit configs); here it inspects URLs passed as
//!   arguments to known fetch commands (`curl`, `wget`, `git`, `pip`,
//!   `pip3`, `npm`, `go`) inside `run:` steps instead.
//! - `self-repository` in zizmor also offers an automated fix; we only
//!   report the finding (no patch/fix machinery exists in this crate).
//! - `unpinned-tools`' coordinate table is a small, manually curated subset
//!   (not zizmor's `ActionCoordinate`/`ControlExpr` engine); the two
//!   `extractions/setup-*` entries are treated as "floating on `latest` or a
//!   missing input" like the other two, rather than reproducing zizmor's
//!   `ControlFieldType::Exact(&["*"])` semantics, which could not be
//!   confirmed from the audit source alone.
//! - `forbidden-uses`/`unpinned-uses`/`typosquat-uses`/`unpinned-tools` are
//!   applied to both step-level `uses:` and job-level reusable-workflow
//!   `uses:` (zizmor's `forbidden-uses`/`unpinned-tools` only look at
//!   steps); this is a deliberate broadening, not a narrowing.
//!
//! `doc` is the parsed GitHub Actions workflow document fed by `audit()`
//! (`jobs.<id>.{uses,container,services,steps[]}` etc.), confirmed with
//! SecCore. A job or step disabled by a literal `if: false` (or an
//! equivalent-after-trimming expression string) suppresses the
//! execution-risk findings below (`forbidden-uses`, `unpinned-uses`,
//! `unpinned-images`, `typosquat-uses`, `unpinned-tools`, `adhoc-packages`,
//! `insecure-url-scheme`) since the reference is never resolved or the
//! script never runs, but not the authoring-quality findings
//! (`obfuscation`, `self-repository`), mirroring `preloop-gha-lint`'s
//! syntax-checks-always-run / execution-advisories-only-when-enabled split.
//! A dynamic `if:` (any expression that isn't the literal `false`) is
//! treated as enabled, never as disabled.

use serde_json::Value;

use crate::{AuditContext, Confidence, Diagnostic, Persona, Severity};

/// Entry point dispatched from `lib.rs`.
pub(crate) fn check(doc: &Value, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };

    let jobs_path = crate::pointer("", "jobs");
    for (job_id, job) in jobs {
        let job_path = crate::pointer(&jobs_path, job_id);
        let job_disabled = is_disabled(job);

        check_uses_bearer(job, &job_path, ctx, job_disabled, out);

        if !job_disabled {
            if let Some(container) = job.get("container") {
                check_container_like(container, &crate::pointer(&job_path, "container"), ctx, out);
            }
            if let Some(services) = job.get("services").and_then(Value::as_object) {
                let services_path = crate::pointer(&job_path, "services");
                for (name, service) in services {
                    check_container_like(service, &crate::pointer(&services_path, name), ctx, out);
                }
            }
        }

        let Some(steps) = job.get("steps").and_then(Value::as_array) else {
            continue;
        };
        let steps_path = crate::pointer(&job_path, "steps");
        for (index, step) in steps.iter().enumerate() {
            let step_path = crate::pointer(&steps_path, &index.to_string());
            let step_disabled = job_disabled || is_disabled(step);

            check_uses_bearer(step, &step_path, ctx, step_disabled, out);

            if !step_disabled {
                check_run_step(step, &step_path, out);
            }
        }
    }
}

/// Only a literal `false` condition disables the findings below. No
/// expression evaluation occurs, even for otherwise constant-looking
/// functions; mirrors `preloop-gha-lint::is_disabled`, reimplemented locally
/// since that helper is crate-private to `preloop-gha-lint`.
fn is_disabled(value: &Value) -> bool {
    match value.get("if") {
        Some(Value::Bool(false)) => true,
        Some(Value::String(text)) => trim_expr_markers(text).eq_ignore_ascii_case("false"),
        _ => false,
    }
}

fn trim_expr_markers(text: &str) -> &str {
    let trimmed = text.trim();
    match trimmed
        .strip_prefix("${{")
        .and_then(|s| s.strip_suffix("}}"))
    {
        Some(inner) => inner.trim(),
        None => trimmed,
    }
}

fn persona_rank(persona: Persona) -> u8 {
    match persona {
        Persona::Regular => 0,
        Persona::Pedantic => 1,
        Persona::Auditor => 2,
    }
}

fn persona_at_least(current: Persona, min: Persona) -> bool {
    persona_rank(current) >= persona_rank(min)
}

fn finding_with(
    id: &str,
    path: &str,
    msg: impl Into<String>,
    help: &str,
    severity: Severity,
    confidence: Confidence,
) -> Diagnostic {
    let mut diagnostic = crate::finding(id, path, msg, help);
    diagnostic.severity = severity;
    diagnostic.confidence = confidence;
    diagnostic
}

// ---------------------------------------------------------------------
// `uses:` parsing
// ---------------------------------------------------------------------

/// A parsed `uses:` reference. `Dynamic` covers both malformed strings and
/// anything containing an unresolved `${{ ... }}` expression; both are
/// treated conservatively as "unknown", never "safe".
enum Uses<'a> {
    Repository {
        owner: &'a str,
        repo: &'a str,
        subpath: Option<&'a str>,
        git_ref: &'a str,
        raw: &'a str,
    },
    Docker {
        image: &'a str,
    },
    Local {
        raw: &'a str,
    },
    Dynamic,
}

fn parse_uses(raw: &str) -> Uses<'_> {
    let trimmed = raw.trim();

    if trimmed.contains("${{") {
        return Uses::Dynamic;
    }
    if let Some(image) = trimmed.strip_prefix("docker://") {
        return Uses::Docker { image };
    }
    if trimmed.starts_with("./") || trimmed.starts_with("$/") {
        return Uses::Local { raw: trimmed };
    }
    if let Some((path, git_ref)) = trimmed.rsplit_once('@')
        && !path.is_empty()
        && !git_ref.is_empty()
        && let mut parts = path.splitn(3, '/')
        && let (Some(owner), Some(repo)) = (parts.next(), parts.next())
        && !owner.is_empty()
        && !repo.is_empty()
    {
        // Keep an explicitly empty subpath (`owner/repo/@ref`):
        // zizmor treats it as an obfuscated empty component.
        let subpath = parts.next();
        return Uses::Repository {
            owner,
            repo,
            subpath,
            git_ref,
            raw: trimmed,
        };
    }

    Uses::Dynamic
}

fn is_commit_sha(git_ref: &str) -> bool {
    git_ref.len() == 40 && git_ref.chars().all(|c| c.is_ascii_hexdigit())
}

fn uses_pattern_matches(pattern: &str, owner: &str, repo: &str, subpath: Option<&str>) -> bool {
    let mut parts = pattern.trim().splitn(3, '/');
    let p_owner = parts.next().unwrap_or("");
    let p_repo = parts.next();
    let p_subpath = parts.next();

    if p_owner != "*" && !p_owner.eq_ignore_ascii_case(owner) {
        return false;
    }
    match p_repo {
        None | Some("*") => true,
        Some(p_repo) => {
            if !p_repo.eq_ignore_ascii_case(repo) {
                return false;
            }
            match p_subpath {
                None | Some("*") => true,
                Some(p_subpath) => subpath.is_some_and(|s| p_subpath.eq_ignore_ascii_case(s)),
            }
        }
    }
}

/// Dispatches every `uses:`-reference check for one job or step object.
/// Obfuscation and self-repository are authoring-quality checks that run
/// even when `disabled` (see module docs); everything else is an
/// execution-risk check and is skipped.
fn check_uses_bearer(
    bearer: &Value,
    path: &str,
    ctx: &AuditContext,
    disabled: bool,
    out: &mut Vec<Diagnostic>,
) {
    let Some(raw) = bearer.get("uses").and_then(Value::as_str) else {
        return;
    };
    let uses_path = crate::pointer(path, "uses");
    let uses = parse_uses(raw);

    check_obfuscation(&uses, &uses_path, out);
    check_self_repository(&uses, &uses_path, out);

    if disabled {
        return;
    }

    match &uses {
        Uses::Repository { .. } => {
            check_forbidden_uses(&uses, &uses_path, ctx, out);
            check_unpinned_uses(&uses, &uses_path, out);
            check_typosquat_uses(&uses, &uses_path, ctx, out);
            check_unpinned_tools(&uses, bearer, path, out);
        }
        Uses::Docker { image, .. } => {
            check_image(image, &uses_path, ctx, out);
        }
        Uses::Local { .. } => {}
        Uses::Dynamic { .. } => {
            out.push(finding_with(
                "unpinned-uses",
                &uses_path,
                "action reference is dynamic; pinning cannot be statically verified",
                "Prefer a literal reference so pinning can be checked automatically.",
                Severity::Info,
                Confidence::Medium,
            ));
        }
    }
}

// ---------------------------------------------------------------------
// obfuscation
// ---------------------------------------------------------------------

fn check_obfuscation(uses: &Uses<'_>, path: &str, out: &mut Vec<Diagnostic>) {
    let Uses::Repository {
        subpath: Some(subpath),
        ..
    } = uses
    else {
        return;
    };

    for component in subpath.split('/') {
        let reason = match component {
            "." => Some("contains a '.' path component"),
            ".." => Some("contains a '..' path component"),
            "" => Some("contains an empty path component"),
            _ => None,
        };
        let Some(reason) = reason else { continue };
        out.push(finding_with(
            "obfuscation",
            path,
            format!("action reference subpath {reason}"),
            "Normalize the path; redundant segments defeat forbidden-uses/unpinned-uses pattern matching and are hard to review.",
            Severity::Info,
            Confidence::High,
        ));
    }
}

// ---------------------------------------------------------------------
// self-repository
// ---------------------------------------------------------------------

fn check_self_repository(uses: &Uses<'_>, path: &str, out: &mut Vec<Diagnostic>) {
    if let Uses::Local { raw } = uses
        && raw.starts_with("./")
    {
        out.push(finding_with(
            "self-repository",
            path,
            "local action/workflow reference uses the relative './...' syntax instead of GitHub's dedicated self-repository syntax",
            "Use '$/...' instead of './...' to reference content in the same repository explicitly.",
            Severity::Info,
            Confidence::High,
        ));
    }
}

// ---------------------------------------------------------------------
// forbidden-uses
// ---------------------------------------------------------------------

fn check_forbidden_uses(
    uses: &Uses<'_>,
    path: &str,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
) {
    let Uses::Repository {
        owner,
        repo,
        subpath,
        raw,
        ..
    } = uses
    else {
        return;
    };

    if ctx
        .forbidden_uses
        .iter()
        .any(|pattern| uses_pattern_matches(pattern, owner, repo, *subpath))
    {
        out.push(finding_with(
            "forbidden-uses",
            path,
            format!("use of `{raw}` is forbidden by policy"),
            "Remove this action or replace it with an approved alternative.",
            Severity::Error,
            Confidence::High,
        ));
        return;
    }

    if !ctx.allowed_uses.is_empty()
        && !ctx
            .allowed_uses
            .iter()
            .any(|pattern| uses_pattern_matches(pattern, owner, repo, *subpath))
    {
        out.push(finding_with(
            "forbidden-uses",
            path,
            format!("use of `{raw}` is not in the configured allow-list"),
            "Add this action to the allow-list or replace it with an approved alternative.",
            Severity::Error,
            Confidence::High,
        ));
    }
}

// ---------------------------------------------------------------------
// unpinned-uses
// ---------------------------------------------------------------------

fn check_unpinned_uses(uses: &Uses<'_>, path: &str, out: &mut Vec<Diagnostic>) {
    let Uses::Repository { git_ref, raw, .. } = uses else {
        return;
    };
    if is_commit_sha(git_ref) {
        return;
    }
    out.push(finding_with(
        "unpinned-uses",
        path,
        format!("`{raw}` is not pinned to a full-length commit SHA"),
        "Pin to the full 40-character commit SHA, not a mutable tag or branch.",
        Severity::Error,
        Confidence::High,
    ));
}

// ---------------------------------------------------------------------
// typosquat-uses
// ---------------------------------------------------------------------

/// Independently compiled sample of well-known `owner/repo` action slugs
/// (lower-cased), used only to flag close near-misses; not comprehensive and
/// not a copy of zizmor's `data/popular-actions.txt`.
const POPULAR_ACTIONS: &[&str] = &[
    "actions/checkout",
    "actions/setup-node",
    "actions/setup-python",
    "actions/setup-go",
    "actions/setup-java",
    "actions/setup-dotnet",
    "actions/cache",
    "actions/upload-artifact",
    "actions/download-artifact",
    "actions/github-script",
    "actions/labeler",
    "actions/stale",
    "actions/create-github-app-token",
    "actions/configure-pages",
    "actions/deploy-pages",
    "actions/dependency-review-action",
    "aws-actions/configure-aws-credentials",
    "azure/login",
    "google-github-actions/auth",
    "docker/build-push-action",
    "docker/login-action",
    "docker/setup-buildx-action",
    "docker/setup-qemu-action",
    "docker/metadata-action",
    "hashicorp/setup-terraform",
    "codecov/codecov-action",
    "dependabot/fetch-metadata",
    "dorny/paths-filter",
    "dtolnay/rust-toolchain",
    "softprops/action-gh-release",
    "peter-evans/create-pull-request",
    "release-drafter/release-drafter",
    "amannn/action-semantic-pull-request",
    "pre-commit/action",
    "astral-sh/setup-uv",
    "mikepenz/action-junit-report",
    "tj-actions/changed-files",
    "jamesives/github-pages-deploy-action",
    "ncipollo/release-action",
    "actions-rs/toolchain",
    "slackapi/slack-github-action",
];

/// Optimal-string-alignment (Levenshtein + adjacent transposition) distance.
fn osa_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }

    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

fn check_typosquat_uses(
    uses: &Uses<'_>,
    path: &str,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
) {
    let Uses::Repository {
        owner, repo, raw, ..
    } = uses
    else {
        return;
    };

    let c_owner = owner.to_lowercase();
    let c_repo = repo.to_lowercase();
    let candidate_slug = format!("{c_owner}/{c_repo}");

    if POPULAR_ACTIONS.contains(&candidate_slug.as_str()) {
        return;
    }
    if ctx
        .allowed_uses
        .iter()
        .any(|pattern| uses_pattern_matches(pattern, owner, repo, None))
    {
        // Explicitly allow-listed by policy; trust the operator's decision
        // rather than second-guessing it as a possible typosquat.
        return;
    }

    for popular in POPULAR_ACTIONS {
        let Some((p_owner, p_repo)) = popular.split_once('/') else {
            continue;
        };
        if p_owner == c_owner && p_repo == c_repo {
            continue;
        }

        if p_repo == c_repo && p_owner != c_owner && osa_distance(p_owner, &c_owner) <= 1 {
            out.push(finding_with(
                "typosquat-uses",
                path,
                format!(
                    "`{c_owner}` closely resembles the popular publisher `{p_owner}` of `{p_repo}` (`{raw}`)"
                ),
                "Verify the publisher name carefully; this may be a typosquat impersonating a popular action.",
                Severity::Warning,
                Confidence::Medium,
            ));
            return;
        }

        if p_owner == c_owner
            && p_repo != c_repo
            && osa_distance(p_repo, &c_repo) <= 1
            && persona_at_least(ctx.persona, Persona::Pedantic)
        {
            out.push(finding_with(
                "typosquat-uses",
                path,
                format!(
                    "`{c_repo}` closely resembles the popular action `{p_owner}/{p_repo}` (`{raw}`)"
                ),
                "Verify the action name carefully; this may be a typosquat of a popular action.",
                Severity::Warning,
                Confidence::Medium,
            ));
            return;
        }
    }
}

// ---------------------------------------------------------------------
// unpinned-tools
// ---------------------------------------------------------------------

struct ToolCoordinate {
    slug: &'static str,
    field: &'static str,
    floating: &'static [&'static str],
}

const TOOL_COORDINATES: &[ToolCoordinate] = &[
    ToolCoordinate {
        slug: "aquasecurity/setup-trivy",
        field: "version",
        floating: &["latest"],
    },
    ToolCoordinate {
        slug: "1password/load-secrets-action",
        field: "version",
        floating: &["latest"],
    },
    ToolCoordinate {
        slug: "extractions/setup-just",
        field: "just-version",
        floating: &["latest"],
    },
    ToolCoordinate {
        slug: "extractions/setup-crate",
        field: "version",
        floating: &["latest"],
    },
];

fn check_unpinned_tools(uses: &Uses<'_>, bearer: &Value, path: &str, out: &mut Vec<Diagnostic>) {
    let Uses::Repository { owner, repo, .. } = uses else {
        return;
    };
    let slug = format!("{}/{}", owner.to_lowercase(), repo.to_lowercase());
    let Some(coordinate) = TOOL_COORDINATES.iter().find(|c| c.slug == slug) else {
        return;
    };

    let field_value = bearer
        .get("with")
        .and_then(Value::as_object)
        .and_then(|with| with.get(coordinate.field));

    match field_value {
        None => {
            out.push(finding_with(
                "unpinned-tools",
                &crate::pointer(path, "uses"),
                format!(
                    "omitting `with.{}` on `{}` implicitly selects an unpinned tool version",
                    coordinate.field, coordinate.slug
                ),
                "Set this input to a specific, pinned tool version.",
                Severity::Warning,
                Confidence::High,
            ));
        }
        Some(Value::String(value)) if value.contains("${{") => {
            out.push(finding_with(
                "unpinned-tools",
                &crate::pointer(&crate::pointer(path, "with"), coordinate.field),
                format!(
                    "`with.{}` is dynamic; the installed tool version may be unpinned",
                    coordinate.field
                ),
                "Pin to a literal, specific tool version.",
                Severity::Warning,
                Confidence::Medium,
            ));
        }
        Some(Value::String(value))
            if coordinate
                .floating
                .iter()
                .any(|f| f.eq_ignore_ascii_case(value)) =>
        {
            out.push(finding_with(
                "unpinned-tools",
                &crate::pointer(&crate::pointer(path, "with"), coordinate.field),
                format!(
                    "`with.{}: {value}` selects an unpinned tool version",
                    coordinate.field
                ),
                "Pin to a literal, specific tool version.",
                Severity::Warning,
                Confidence::High,
            ));
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------
// unpinned-images (container/service/docker-step image references)
// ---------------------------------------------------------------------

fn check_container_like(value: &Value, path: &str, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    match value {
        Value::String(image) => check_image(image, path, ctx, out),
        Value::Object(map) => {
            if let Some(Value::String(image)) = map.get("image") {
                check_image(image, &crate::pointer(path, "image"), ctx, out);
            }
        }
        _ => {}
    }
}

enum DockerPin<'a> {
    Digest,
    LatestTag,
    Tag(&'a str),
    NoTag,
}

fn is_valid_digest(candidate: &str) -> bool {
    match candidate.split_once(':') {
        Some((algo, hex)) => {
            !algo.is_empty()
                && algo.chars().all(|c| c.is_ascii_alphanumeric())
                && hex.len() >= 32
                && hex.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => false,
    }
}

fn classify_docker_image(image: &str) -> DockerPin<'_> {
    if let Some(at) = image.rfind('@') {
        let digest = &image[at + 1..];
        if is_valid_digest(digest) {
            return DockerPin::Digest;
        }
    }

    let search_from = image.rfind('/').map_or(0, |i| i + 1);
    if let Some(colon_rel) = image[search_from..].find(':') {
        let tag = &image[search_from + colon_rel + 1..];
        return if tag == "latest" {
            DockerPin::LatestTag
        } else {
            DockerPin::Tag(tag)
        };
    }

    DockerPin::NoTag
}

fn check_image(image: &str, path: &str, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    let trimmed = image.trim();
    if trimmed.is_empty() {
        return;
    }
    if trimmed.contains("${{") {
        out.push(finding_with(
            "unpinned-images",
            path,
            "container image reference is dynamic; pinning cannot be statically verified",
            "Prefer a literal image reference pinned to a digest.",
            Severity::Info,
            Confidence::Medium,
        ));
        return;
    }

    let image_ref = trimmed.strip_prefix("docker://").unwrap_or(trimmed);
    match classify_docker_image(image_ref) {
        DockerPin::Digest => {}
        DockerPin::LatestTag => out.push(finding_with(
            "unpinned-images",
            path,
            format!("container image `{image_ref}` uses the floating 'latest' tag"),
            "Pin the image to an immutable digest (`name@sha256:...`).",
            Severity::Warning,
            Confidence::High,
        )),
        DockerPin::NoTag => out.push(finding_with(
            "unpinned-images",
            path,
            format!("container image `{image_ref}` has no tag or digest and defaults to 'latest'"),
            "Pin the image to an immutable digest (`name@sha256:...`).",
            Severity::Warning,
            Confidence::High,
        )),
        DockerPin::Tag(tag) => {
            if persona_at_least(ctx.persona, Persona::Pedantic) {
                out.push(finding_with(
                    "unpinned-images",
                    path,
                    format!("container image `{image_ref}` is pinned to tag `{tag}`, not a digest"),
                    "Pin the image to an immutable digest (`name@sha256:...`); tags can be repointed by the registry owner.",
                    Severity::Info,
                    Confidence::Medium,
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------
// adhoc-packages / insecure-url-scheme (`run:` script analysis)
// ---------------------------------------------------------------------

/// Splits a shell `run:` body into simple-command argument lists, respecting
/// basic single/double quoting, backslash escapes, `#` comments, and the
/// `;`, `&&`, `||`, `|`, newline command separators. This is a lightweight
/// heuristic, not a full shell parser: it does not resolve variable
/// expansion, here-docs, or nested subshells, and may under- or over-split
/// unusual scripts. GitHub Actions expression placeholders (`${{ ... }}`)
/// are left as opaque word fragments, which is sufficient here since no
/// check below needs to resolve them to match.
fn simple_commands(script: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut current = Vec::new();
    let mut word = String::new();
    let mut has_word = false;
    let mut in_single = false;
    let mut in_double = false;

    let mut chars = script.chars().peekable();
    while let Some(c) = chars.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
            } else {
                word.push(c);
                has_word = true;
            }
            continue;
        }
        if in_double {
            match c {
                '"' => in_double = false,
                '\\' => {
                    if let Some(next) = chars.next() {
                        word.push(next);
                        has_word = true;
                    }
                }
                _ => {
                    word.push(c);
                    has_word = true;
                }
            }
            continue;
        }
        match c {
            '\'' => {
                in_single = true;
                has_word = true;
            }
            '"' => {
                in_double = true;
                has_word = true;
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    has_word = true;
                }
            }
            '#' => {
                for d in chars.by_ref() {
                    if d == '\n' {
                        break;
                    }
                }
                if has_word {
                    current.push(std::mem::take(&mut word));
                    has_word = false;
                }
                if !current.is_empty() {
                    commands.push(std::mem::take(&mut current));
                }
            }
            '&' => {
                if chars.peek() == Some(&'&') {
                    chars.next();
                }
                if has_word {
                    current.push(std::mem::take(&mut word));
                    has_word = false;
                }
                if !current.is_empty() {
                    commands.push(std::mem::take(&mut current));
                }
            }
            '|' => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                if has_word {
                    current.push(std::mem::take(&mut word));
                    has_word = false;
                }
                if !current.is_empty() {
                    commands.push(std::mem::take(&mut current));
                }
            }
            ';' | '\n' => {
                if has_word {
                    current.push(std::mem::take(&mut word));
                    has_word = false;
                }
                if !current.is_empty() {
                    commands.push(std::mem::take(&mut current));
                }
            }
            c if c.is_whitespace() => {
                if has_word {
                    current.push(std::mem::take(&mut word));
                    has_word = false;
                }
            }
            _ => {
                word.push(c);
                has_word = true;
            }
        }
    }
    if has_word {
        current.push(word);
    }
    if !current.is_empty() {
        commands.push(current);
    }

    commands
}

fn command_name(argv: &[String]) -> Option<&str> {
    argv.first()
        .map(|cmd| cmd.rsplit('/').next().unwrap_or(cmd.as_str()))
}

/// Ported from zizmor's `AdhocPackages::is_adhoc_install_command`
/// (gem/bundle/npm/yarn/pnpm branches), plus a `pip`/`pip3` addition.
fn is_adhoc_install_command(argv: &[String]) -> bool {
    let Some(cmd) = command_name(argv) else {
        return false;
    };
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();

    match cmd {
        "gem" => {
            args.iter().any(|a| *a == "install" || *a == "i")
                && args.iter().any(|a| !a.starts_with('-'))
        }
        "bundle" => args.contains(&"add") && args.iter().any(|a| !a.starts_with('-')),
        "npm" => {
            let mut rest = args.iter().skip_while(|a| a.starts_with('-'));
            match rest.next().copied() {
                Some(
                    "install" | "i" | "in" | "ins" | "inst" | "insta" | "instal" | "isnt" | "isnta"
                    | "isntal" | "isntall" | "add",
                ) => rest.any(|a| !a.starts_with('-')),
                _ => false,
            }
        }
        "yarn" | "pnpm" => is_yarn_pnpm_add(cmd, &args),
        "pip" | "pip3" => {
            let mut rest = args.iter();
            match rest.next().copied() {
                Some("install") => {
                    let remaining: Vec<&str> = rest.copied().collect();
                    let has_requirement_file = remaining.iter().any(|a| {
                        *a == "-r" || *a == "--requirement" || a.starts_with("--requirement=")
                    });
                    let has_package = remaining.iter().any(|a| !a.starts_with('-'));
                    has_package && !has_requirement_file
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn yarn_pnpm_option_takes_value(cmd: &str, option: &str) -> bool {
    match cmd {
        "pnpm" => matches!(option, "-C" | "--dir" | "-F" | "--filter" | "--filter-prod"),
        "yarn" => matches!(option, "--cwd" | "--mutex" | "--cache-folder"),
        _ => false,
    }
}

fn next_subcommand<'a>(cmd: &str, args: &mut impl Iterator<Item = &'a str>) -> Option<&'a str> {
    while let Some(arg) = args.next() {
        if arg == "--" {
            return args.next();
        }
        if !arg.starts_with('-') {
            return Some(arg);
        }
        if !arg.contains('=') && yarn_pnpm_option_takes_value(cmd, arg) {
            args.next()?;
        }
    }
    None
}

fn is_yarn_pnpm_add(cmd: &str, args: &[&str]) -> bool {
    let mut iter = args.iter().copied();
    let subcommand = match (cmd, next_subcommand(cmd, &mut iter)) {
        ("yarn", Some("workspace")) => {
            iter.next();
            next_subcommand(cmd, &mut iter)
        }
        ("yarn", Some("global")) | ("pnpm", Some("recursive" | "multi" | "m")) => {
            next_subcommand(cmd, &mut iter)
        }
        (_, subcommand) => subcommand,
    };

    matches!(subcommand, Some("add")) && iter.any(|a| !a.starts_with('-'))
}

fn insecure_url_arguments(argv: &[String]) -> Vec<&str> {
    let Some(cmd) = command_name(argv) else {
        return Vec::new();
    };
    if !matches!(cmd, "curl" | "wget" | "git" | "pip" | "pip3" | "npm" | "go") {
        return Vec::new();
    }
    argv[1..]
        .iter()
        .filter(|a| a.starts_with("http://") || a.starts_with("git://"))
        .map(String::as_str)
        .collect()
}

fn check_run_step(step: &Value, path: &str, out: &mut Vec<Diagnostic>) {
    let Some(run) = step.get("run").and_then(Value::as_str) else {
        return;
    };
    let shell = step.get("shell").and_then(Value::as_str).unwrap_or("bash");
    // Only bash-family scripts are tokenized; see module docs for the
    // pwsh/unknown-shell limitation.
    if !matches!(shell, "bash" | "sh" | "zsh" | "") {
        return;
    }

    let run_path = crate::pointer(path, "run");
    for argv in simple_commands(run) {
        if is_adhoc_install_command(&argv) {
            out.push(finding_with(
                "adhoc-packages",
                &run_path,
                format!("`{}` installs a package outside of a lockfile", argv.join(" ")),
                "Install dependencies through a lockfile (package-lock.json, Gemfile.lock, requirements.txt) instead of an ad-hoc install command.",
                Severity::Info,
                Confidence::High,
            ));
        }
        for url in insecure_url_arguments(&argv) {
            out.push(finding_with(
                "insecure-url-scheme",
                &run_path,
                format!("fetch command uses an insecure URL scheme: `{url}`"),
                "Use an https:// URL instead of a plaintext http:// or git:// transport.",
                Severity::Warning,
                Confidence::High,
            ));
        }
    }
}

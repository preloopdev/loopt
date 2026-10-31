//! Native reimplementations of eleven zizmor (MIT) policy-shaped audits:
//! `excessive-permissions`, `use-trusted-publishing`, `self-hosted-runner`,
//! `undocumented-permissions`, `anonymous-definition`, `dependabot-execution`,
//! `dependabot-cooldown`, `concurrency-limits`, `misfeature`,
//! `superfluous-actions`, and `github-app`.
//!
//! Scope and attribution: the structural logic in each check below was
//! studied from, and in several cases closely mirrors the decision tables
//! of, the MIT-licensed zizmor audits at
//! `crates/zizmor/src/audit/{excessive_permissions,use_trusted_publishing,
//! self_hosted_runner,undocumented_permissions,anonymous_definition,
//! dependabot_execution,dependabot_cooldown,concurrency_limits,misfeature,
//! superfluous_actions,github_app}.rs` in
//! <https://github.com/zizmorcore/zizmor> (MIT licensed; see that project's
//! `LICENSE`). No zizmor code is linked into this crate; this is an
//! independent, offline reimplementation against `serde_json::Value` rather
//! than zizmor's typed `github_actions_models` tree, and intentionally
//! narrows several checks (documented per-function below) where zizmor
//! relies on inputs we don't have: a shared per-repository audit config
//! (self-hosted runner-group allow/deny lists), a generated FST of the full
//! GitHub-hosted-runner label catalog, YAML comment provenance, or a
//! tree-sitter shell-script parser.
//!
//! `doc` is always the parsed GitHub Actions *workflow* document fed by
//! `audit()` (`on`/`permissions`/`jobs.<id>.steps[]`/…) — confirmed with
//! SecCore. `dependabot-execution` and `dependabot-cooldown` instead key off
//! a top-level Dependabot `updates` array, which this entrypoint never
//! supplies today; their `check` logic is implemented and unit-tested
//! directly against synthetic Dependabot-shaped `Value`s so it's correct and
//! ready to wire in, but through `audit()` they are permanently no-ops until
//! a Dependabot-config entrypoint exists. See the crate-level report to Main
//! for this parity gap — it is not something this module can resolve alone.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::{AuditContext, Confidence, Diagnostic, Persona, Severity};

/// Relative verbosity of the three personas, matching zizmor's own
/// `--pedantic`/`--auditor` escalation: `Regular` findings always show;
/// `Pedantic` findings need `ctx.persona` to be `Pedantic` or `Auditor`;
/// `Auditor` findings need `ctx.persona == Auditor`. `Persona` has no
/// `Ord` impl, so this ranks manually.
fn persona_rank(p: Persona) -> u8 {
    match p {
        Persona::Regular => 0,
        Persona::Pedantic => 1,
        Persona::Auditor => 2,
    }
}

fn persona_enabled(ctx: &AuditContext, required: Persona) -> bool {
    persona_rank(ctx.persona) >= persona_rank(required)
}

/// Entry point dispatched from `lib.rs`. Each sub-check independently
/// detects whether `doc` has the shape it needs and no-ops otherwise.
pub(crate) fn check(doc: &Value, source: &str, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    let is_workflow = doc.get("jobs").is_some_and(Value::is_object);
    if is_workflow {
        anonymous_definition(doc, ctx, out);
        excessive_permissions(doc, ctx, out);
        undocumented_permissions(doc, source, ctx, out);
        concurrency_limits(doc, ctx, out);
        self_hosted_runner(doc, ctx, out);
        for_each_step(doc, &mut |step, path| {
            use_trusted_publishing_step(step, path, out);
            misfeature_step(step, path, ctx, out);
            superfluous_actions_step(step, path, ctx, out);
            github_app_step(step, path, out);
        });
    }
    if doc.get("updates").is_some_and(Value::is_array) {
        dependabot_execution(doc, out);
        dependabot_cooldown(doc, out);
    }
}

// ---------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------

/// Mirrors `preloop-gha-lint`'s `if:` disablement semantics: a literal
/// `false`, or an expression that trims (after stripping `${{ }}`) to
/// `false` case-insensitively. Duplicated locally rather than imported,
/// since lint-core's `is_disabled` is crate-private to `preloop-gha-lint`.
fn is_disabled(value: &Value) -> bool {
    match value.get("if") {
        Some(Value::Bool(false)) => true,
        Some(Value::String(text)) => trim_expr(text).eq_ignore_ascii_case("false"),
        _ => false,
    }
}

/// Strip `${{` / `}}` delimiters, mirroring
/// `preloop_gha_expressions::trim_expression_markers`.
fn trim_expr(input: &str) -> &str {
    let value = input.trim();
    match value.strip_prefix("${{").and_then(|s| s.strip_suffix("}}")) {
        Some(inner) => inner.trim(),
        None => value,
    }
}

/// `true` if `value` is a non-empty string containing a `${{ ... }}`
/// expression anywhere (not necessarily the whole value).
fn has_expression(value: &Value) -> bool {
    matches!(value, Value::String(s) if s.contains("${{"))
}

/// GitHub Actions' "actions/toolkit" boolean coercion: a native JSON bool,
/// or the strings `true`/`false` case-insensitively. Returns `None` for
/// anything else (including unresolved expressions), which callers treat
/// as "unknown, don't flag" rather than fabricating a verdict.
fn toolkit_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::String(s) if s.eq_ignore_ascii_case("true") => Some(true),
        Value::String(s) if s.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

fn child(parent: &str, key: &str) -> String {
    crate::pointer(parent, key)
}

/// Iterate every step across every job's `steps:` array, non-reusable jobs
/// only (reusable `uses:` jobs have no step list), skipping steps whose
/// `if:` is statically `false`. `f` receives the step object and its JSON
/// pointer.
fn for_each_step(doc: &Value, f: &mut dyn FnMut(&Value, &str)) {
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        if !job.is_object() || is_disabled(job) {
            continue;
        }
        let Some(steps) = job.get("steps").and_then(Value::as_array) else {
            continue;
        };
        let job_path = child("/jobs", job_id);
        let steps_path = child(&job_path, "steps");
        for (idx, step) in steps.iter().enumerate() {
            if !step.is_object() || is_disabled(step) {
                continue;
            }
            let step_path = format!("{steps_path}/{idx}");
            f(step, &step_path);
        }
    }
}

/// `uses:` as `(owner, repo)` ignoring the ref and any subpath, for a
/// repository-form `uses:` (`owner/repo[/sub]@ref`). Docker (`docker://…`)
/// and local (`./…`) forms return `None`.
fn uses_owner_repo(uses: &str) -> Option<(&str, &str)> {
    if uses.starts_with("./") || uses.starts_with("docker://") {
        return None;
    }
    let without_ref = uses.split('@').next().unwrap_or(uses);
    let mut parts = without_ref.splitn(3, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

fn uses_matches(uses: &str, owner_repo: &str) -> bool {
    match uses_owner_repo(uses) {
        Some((owner, repo)) => {
            let candidate = format!("{owner}/{repo}");
            candidate.eq_ignore_ascii_case(owner_repo)
        }
        None => false,
    }
}

/// `true` for a reusable-workflow-call job (`uses: ./.github/workflows/…`
/// or `owner/repo/.github/workflows/…@ref`), which has no `steps:`/
/// `runs-on:` of its own — the called workflow owns those.
fn is_reusable_call_job(job: &Value) -> bool {
    job.get("uses").and_then(Value::as_str).is_some()
}

// ---------------------------------------------------------------------
// anonymous-definition
// ---------------------------------------------------------------------

/// Flags a workflow with no top-level `name:`, and jobs with no `name:`.
/// Matches zizmor's `anonymous_definition` severities and persona: both
/// findings are `Persona::Pedantic` (GitHub's UI falls back to the
/// filename/job id, so this is cosmetic rather than a security concern).
fn anonymous_definition(doc: &Value, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    if !persona_enabled(ctx, Persona::Pedantic) {
        return;
    }
    if doc.get("name").and_then(Value::as_str).is_none() {
        let mut d = crate::finding(
            "anonymous-definition",
            "",
            "workflow has no `name:`",
            "use `name: ...` to give this workflow a name so it's easy to find in the GitHub UI",
        );
        d.severity = Severity::Info;
        d.confidence = Confidence::High;
        out.push(d);
    }

    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        if !job.is_object() {
            continue;
        }
        if job.get("name").and_then(Value::as_str).is_none() {
            let path = child("/jobs", job_id);
            let mut d = crate::finding(
                "anonymous-definition",
                &path,
                format!("job `{job_id}` has no `name:`"),
                "use `name: ...` to give this job a name",
            );
            d.severity = Severity::Info;
            d.confidence = Confidence::High;
            out.push(d);
        }
    }
}

// ---------------------------------------------------------------------
// excessive-permissions
// ---------------------------------------------------------------------

/// Severity by GITHUB_TOKEN scope when granted `write`, same subjective
/// mapping zizmor uses.
fn known_permission_severity(name: &str) -> Severity {
    match name {
        "actions" | "attestations" | "contents" | "deployments" | "id-token" | "issues"
        | "packages" | "pages" | "pull-requests" => Severity::Error,
        "artifact-metadata"
        | "checks"
        | "code-quality"
        | "discussions"
        | "repository-projects"
        | "security-events" => Severity::Warning,
        // `models`/`statuses` write access has little practical blast radius.
        "models" | "statuses" => Severity::Info,
        _ => Severity::Warning,
    }
}

/// `permissions:` may be the string `read-all`/`write-all`, an empty/absent
/// block (implicit default — broad, but *which* scopes is undiscoverable
/// without GitHub's default-token-permissions org/repo setting), or an
/// explicit `{scope: level}` map.
enum Permissions<'a> {
    Default,
    ReadAll,
    WriteAll,
    Explicit(&'a serde_json::Map<String, Value>),
}

/// `permissions: {}` is an explicit, maximally-restrictive grant (no
/// GITHUB_TOKEN scopes at all) — semantically the opposite of an absent
/// `permissions:` block, which falls back to GitHub's broader implicit
/// default. The two must not collapse together.
fn parse_permissions(value: Option<&Value>) -> Permissions<'_> {
    match value {
        None => Permissions::Default,
        Some(Value::String(s)) if s.eq_ignore_ascii_case("read-all") => Permissions::ReadAll,
        Some(Value::String(s)) if s.eq_ignore_ascii_case("write-all") => Permissions::WriteAll,
        Some(Value::Object(map)) => Permissions::Explicit(map),
        // Expression-valued or otherwise unrecognized `permissions:` can't
        // be statically classified; treat like an absent block rather than
        // fabricate a verdict.
        Some(_) => Permissions::Default,
    }
}

fn excessive_permissions(doc: &Value, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    let workflow_perms = parse_permissions(doc.get("permissions"));
    let explicit_parent = !matches!(workflow_perms, Permissions::Default);

    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };

    // Mirrors zizmor's `workflow_finding_persona`: top-level permissions
    // are only cosmetic noise (Pedantic) when every job either has its own
    // explicit permissions already, there's only one job to begin with, or
    // this workflow is reusable-only (so the caller owns permissions).
    let all_jobs_have_permissions = jobs.values().all(|job| {
        !matches!(
            parse_permissions(job.get("permissions")),
            Permissions::Default
        )
    });
    let workflow_is_reusable_only = is_reusable_only(doc);
    let workflow_persona =
        if jobs.len() == 1 || all_jobs_have_permissions || workflow_is_reusable_only {
            Persona::Pedantic
        } else {
            Persona::Regular
        };

    if persona_enabled(ctx, workflow_persona) {
        // Top-level findings.
        let perms_path = child("", "permissions");
        match &workflow_perms {
            Permissions::Default => {
                let mut d = crate::finding(
                    "excessive-permissions",
                    "",
                    "no top-level `permissions:` block; the default GITHUB_TOKEN permissions apply",
                    "add an explicit `permissions:` block scoped to the minimum this workflow needs",
                );
                d.severity = Severity::Warning;
                d.confidence = Confidence::Medium;
                out.push(d);
            }
            Permissions::ReadAll => {
                let mut d = crate::finding(
                    "excessive-permissions",
                    &perms_path,
                    "top-level `permissions: read-all` grants read access to every scope",
                    "scope `permissions:` to only the read access this workflow needs",
                );
                d.severity = Severity::Warning;
                d.confidence = Confidence::High;
                out.push(d);
            }
            Permissions::WriteAll => {
                let mut d = crate::finding(
                    "excessive-permissions",
                    &perms_path,
                    "top-level `permissions: write-all` grants write access to every scope",
                    "scope `permissions:` to only the write access this workflow needs",
                );
                d.severity = Severity::Error;
                d.confidence = Confidence::High;
                out.push(d);
            }
            Permissions::Explicit(map) => {
                for (name, level) in map.iter() {
                    if level.as_str() != Some("write") {
                        continue;
                    }
                    let scope_path = child(&perms_path, name);
                    let mut d = crate::finding(
                        "excessive-permissions",
                        &scope_path,
                        format!("`{name}: write` is overly broad at the workflow level"),
                        "move this grant to the job(s) that actually need it",
                    );
                    d.severity = known_permission_severity(name);
                    d.confidence = Confidence::High;
                    out.push(d);
                }
            }
        }
    }

    // Per-job findings. Explicit job-level permission maps can't be judged
    // (we have no minimum-permission-set table per action), matching
    // zizmor's own scope limitation there.
    for (job_id, job) in jobs {
        if !job.is_object() {
            continue;
        }
        // Reusable-call jobs always get Regular (the caller owns their
        // permissions regardless of this workflow's reusable-only status);
        // normal jobs follow the workflow's reusable-only status.
        let job_persona = if is_reusable_call_job(job) {
            Persona::Regular
        } else if workflow_is_reusable_only {
            Persona::Pedantic
        } else {
            Persona::Regular
        };
        if !persona_enabled(ctx, job_persona) {
            continue;
        }
        let job_perms = parse_permissions(job.get("permissions"));
        let job_path = child("/jobs", job_id);
        let job_perms_path = child(&job_path, "permissions");
        match job_perms {
            Permissions::Default if !explicit_parent => {
                let mut d = crate::finding(
                    "excessive-permissions",
                    &job_path,
                    format!(
                        "job `{job_id}` has no `permissions:` and the workflow has none either; \
                         the default GITHUB_TOKEN permissions apply"
                    ),
                    "add a `permissions:` block to this job or to the workflow",
                );
                d.severity = Severity::Warning;
                d.confidence = Confidence::Medium;
                out.push(d);
            }
            Permissions::Default => {}
            Permissions::ReadAll => {
                let mut d = crate::finding(
                    "excessive-permissions",
                    &job_perms_path,
                    format!("job `{job_id}` uses `permissions: read-all`"),
                    "scope this job's `permissions:` to only the read access it needs",
                );
                d.severity = Severity::Warning;
                d.confidence = Confidence::High;
                out.push(d);
            }
            Permissions::WriteAll => {
                let mut d = crate::finding(
                    "excessive-permissions",
                    &job_perms_path,
                    format!("job `{job_id}` uses `permissions: write-all`"),
                    "scope this job's `permissions:` to only the write access it needs",
                );
                d.severity = Severity::Error;
                d.confidence = Confidence::High;
                out.push(d);
            }
            Permissions::Explicit(_) => {}
        }
    }
}

// ---------------------------------------------------------------------
// undocumented-permissions
// ---------------------------------------------------------------------

/// Explicit permission grants need an explanatory comment. The JSON view
/// cannot carry comments, so this rule reads the original YAML source and
/// maps `permissions:` blocks to their scopes in declaration order. A
/// flow-style `permissions: {...}` block has no per-scope line to attach a
/// comment to and is left unknown rather than reported.
///
/// `contents: read` is skipped, matching the reference rule: it is common
/// and self-explanatory.
fn undocumented_permissions(
    doc: &Value,
    source: &str,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
) {
    if !persona_enabled(ctx, Persona::Pedantic) {
        return;
    }

    let blocks = permission_blocks(source);
    let mut entries: Vec<(String, &Value)> = Vec::new();
    if let Some(permissions) = doc.get("permissions") {
        entries.push(("".to_owned(), permissions));
    }
    if let Some(jobs) = doc.get("jobs").and_then(Value::as_object) {
        for (job_id, job) in jobs {
            if let Some(permissions) = job.get("permissions") {
                entries.push((child("/jobs", job_id), permissions));
            }
        }
    }

    // A flow-style block has no per-scope lines, so it is omitted from
    // `blocks`. If the counts disagree the source mapping is ambiguous and
    // the rule stays silent rather than attributing comments to the wrong
    // grant.
    if blocks.len() != entries.len() {
        return;
    }

    for ((pointer, permissions), block) in entries.iter().zip(blocks.iter()) {
        let Some(map) = permissions.as_object() else {
            continue;
        };
        if map.is_empty() {
            continue;
        }
        for (scope, level) in map {
            if scope == "contents" && level.as_str() == Some("read") {
                continue;
            }
            let Some(grant) = block.grants.iter().find(|grant| grant.scope == *scope) else {
                continue;
            };
            if grant.documented {
                continue;
            }
            let path = child(&child(pointer, "permissions"), scope);
            let mut d = crate::finding(
                "undocumented-permissions",
                &path,
                format!("`{scope}: {level}` needs an explanatory comment"),
                "add a comment above the grant explaining why this workflow or job needs it",
            );
            d.severity = Severity::Info;
            d.confidence = Confidence::High;
            d.line = Some(grant.line);
            out.push(d);
        }
    }
}

struct PermissionBlock {
    grants: Vec<PermissionGrant>,
}

struct PermissionGrant {
    scope: String,
    line: usize,
    documented: bool,
}

/// Collect block-style `permissions:` sections in source order.
fn permission_blocks(source: &str) -> Vec<PermissionBlock> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim_start();
        let indent = raw.len() - trimmed.len();
        let Some(rest) = trimmed.strip_prefix("permissions:") else {
            i += 1;
            continue;
        };
        if rest.trim_start().starts_with('{') {
            // Flow style: no per-scope source lines to comment.
            i += 1;
            continue;
        }
        let mut block = PermissionBlock { grants: Vec::new() };
        let mut j = i + 1;
        while j < lines.len() {
            let raw = lines[j];
            let trimmed = raw.trim_start();
            if trimmed.is_empty() {
                j += 1;
                continue;
            }
            let child_indent = raw.len() - trimmed.len();
            if child_indent <= indent {
                break;
            }
            if trimmed.starts_with('#') {
                j += 1;
                continue;
            }
            if let Some((scope, value)) = trimmed.split_once(':') {
                let scope = scope.trim();
                if !scope.is_empty() {
                    block.grants.push(PermissionGrant {
                        scope: scope.to_owned(),
                        line: j + 1,
                        documented: value.contains('#')
                            || preceding_comment(&lines, j, child_indent),
                    });
                }
            }
            j += 1;
        }
        blocks.push(block);
        i = j;
    }
    blocks
}

fn preceding_comment(lines: &[&str], line: usize, indent: usize) -> bool {
    let mut previous = line;
    while previous > 0 {
        previous -= 1;
        let raw = lines[previous];
        let trimmed = raw.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let previous_indent = raw.len() - trimmed.len();
        if previous_indent < indent {
            return false;
        }
        return trimmed.starts_with('#');
    }
    false
}

// ---------------------------------------------------------------------
// concurrency-limits
// ---------------------------------------------------------------------

/// `true` if `job`'s trigger set makes it reusable-only (its *only*
/// trigger is `workflow_call`), in which case the caller is expected to
/// own concurrency and this workflow nagging about it causes real bugs
/// (deadlocks / premature cancellation of the caller's own group).
fn is_reusable_only(doc: &Value) -> bool {
    match doc.get("on") {
        Some(Value::Object(map)) => map.len() == 1 && map.contains_key("workflow_call"),
        Some(Value::String(s)) => s == "workflow_call",
        _ => false,
    }
}

fn concurrency_shape(value: Option<&Value>) -> Option<&'static str> {
    match value? {
        Value::String(_) => Some("bare"),
        Value::Object(map) => {
            if map.contains_key("cancel-in-progress") {
                Some("complete")
            } else {
                Some("bare")
            }
        }
        _ => None,
    }
}

/// All three finding shapes are `Persona::Pedantic` in zizmor (cosmetic
/// best-practice nudge, not a direct vulnerability).
fn concurrency_limits(doc: &Value, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    if is_reusable_only(doc) || !persona_enabled(ctx, Persona::Pedantic) {
        return;
    }

    match concurrency_shape(doc.get("concurrency")) {
        Some("bare") => {
            let mut d = crate::finding(
                "concurrency-limits",
                &child("", "concurrency"),
                "workflow `concurrency:` has no `cancel-in-progress`",
                "add `cancel-in-progress: true` (or an explicit `false` if that's intentional)",
            );
            d.severity = Severity::Info;
            d.confidence = Confidence::High;
            out.push(d);
            return;
        }
        Some("complete") => return,
        None => {}
        _ => unreachable!(),
    }

    // Workflow-level `concurrency:` is entirely absent: look at jobs.
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    let mut missing_cancel = Vec::new();
    let mut missing_entirely = Vec::new();
    for (job_id, job) in jobs {
        if !job.is_object() {
            continue;
        }
        match concurrency_shape(job.get("concurrency")) {
            Some("bare") => missing_cancel.push(job_id.as_str()),
            None => missing_entirely.push(job_id.as_str()),
            _ => {}
        }
    }

    if !missing_cancel.is_empty() {
        let names = missing_cancel.join(", ");
        let mut d = crate::finding(
            "concurrency-limits",
            &child("", "on"),
            format!(
                "workflow has no top-level `concurrency:`, and job(s) [{names}] set \
                 `concurrency:` without `cancel-in-progress`"
            ),
            "add `cancel-in-progress: true` to each job's `concurrency:` block",
        );
        d.severity = Severity::Info;
        d.confidence = Confidence::High;
        out.push(d);
    }
    if !missing_entirely.is_empty() {
        let names = missing_entirely.join(", ");
        let mut d = crate::finding(
            "concurrency-limits",
            &child("", "on"),
            format!(
                "workflow has no top-level `concurrency:`, and job(s) [{names}] set none either"
            ),
            "add a `concurrency:` block (workflow- or job-level) to prevent overlapping runs",
        );
        d.severity = Severity::Info;
        d.confidence = Confidence::High;
        out.push(d);
    }
}

// ---------------------------------------------------------------------
// self-hosted-runner
// ---------------------------------------------------------------------

/// GitHub-hosted runner label families, as regexes. KNOWN LIMITATION:
/// zizmor ships a generated FST of GitHub's complete, versioned
/// hosted-runner-label catalog (built from GitHub's `runner-images`
/// metadata at `zizmor`'s build time). We have no offline equivalent of
/// that dataset, so this is a hand-maintained pattern set covering the
/// common `ubuntu-*`/`windows-*`/`macos-*` families (including the
/// `-arm`/`-xlarge`/N-core larger-runner suffixes). A label that's
/// GitHub-hosted but doesn't match one of these patterns (e.g. a very new
/// or very old runner image) will be reported as "unrecognized" at Medium
/// confidence rather than silently passed.
fn is_known_github_hosted_label(label: &str) -> bool {
    static UBUNTU: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^ubuntu-(latest|\d{2}\.\d{2})(-(arm|arm64|\d+-cores))?$")
            .expect("static regex")
    });
    static WINDOWS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^windows-(latest|\d{4}|11-arm)(-(arm|\d+-cores))?$")
            .expect("static regex")
    });
    static MACOS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^macos-(latest(-(xl|large|xlarge))?|\d{1,2}(-(xl|large|xlarge|arm64))?)$",
        )
        .expect("static regex")
    });
    UBUNTU.is_match(label) || WINDOWS.is_match(label) || MACOS.is_match(label)
}

/// Known managed/ephemeral third-party runner providers, matched by label
/// prefix. These are cloud-managed, per-run-ephemeral runner fleets
/// (unlike a traditional persistent self-hosted box), so zizmor doesn't
/// flag them. This is a small, hand-maintained list, not exhaustive.
const THIRD_PARTY_RUNNER_PREFIXES: &[&str] = &[
    "namespace-",
    "runs-on=",
    "warp-",
    "buildjet-",
    "blacksmith-",
    "depot-",
];

fn is_known_third_party_label(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    THIRD_PARTY_RUNNER_PREFIXES
        .iter()
        .any(|prefix| l.starts_with(prefix))
}

/// All findings here are `Persona::Auditor` in zizmor — self-hosted
/// runners are a real risk, but one this audit can't confirm (ephemeral
/// vs. persistent, network-isolated vs. not) without live infrastructure
/// knowledge, so zizmor keeps it out of the default/regular finding set.
fn self_hosted_runner(doc: &Value, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    if !persona_enabled(ctx, Persona::Auditor) {
        return;
    }
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        if !job.is_object() || is_reusable_call_job(job) {
            continue; // reusable-call jobs have no `runs-on`.
        }
        let Some(runs_on) = job.get("runs-on") else {
            continue;
        };
        let job_path = child("/jobs", job_id);
        let runs_on_path = child(&job_path, "runs-on");
        self_hosted_runner_value(job_id, &runs_on_path, runs_on, out);
    }
}

fn self_hosted_runner_value(job_id: &str, path: &str, runs_on: &Value, out: &mut Vec<Diagnostic>) {
    match runs_on {
        Value::String(s) => {
            self_hosted_runner_label_set(job_id, path, std::slice::from_ref(s), out)
        }
        Value::Array(items) => {
            let labels: Vec<String> = items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            if labels.len() == items.len() {
                self_hosted_runner_label_set(job_id, path, &labels, out);
            } else if items.iter().any(has_expression) {
                push_indeterminate(job_id, path, out);
            }
        }
        Value::Object(map) => {
            if let Some(group) = map.get("group") {
                if has_expression(group) {
                    push_indeterminate(job_id, path, out);
                } else if let Some(name) = group.as_str() {
                    // No allow/deny-group config is available to us, so every
                    // runner-group reference is reported — GitHub's own
                    // larger-runner groups use this same shape, which we
                    // can't distinguish from a self-hosted group here.
                    let mut d = crate::finding(
                        "self-hosted-runner",
                        path,
                        format!(
                            "job `{job_id}` runs in the runner group `{name}`; runner groups \
                             usually imply self-hosted runners (GitHub's own larger-runner \
                             groups are the one exception we can't distinguish from here)"
                        ),
                        "confirm this group is a GitHub-managed larger-runner group, not a \
                         self-hosted pool",
                    );
                    d.severity = Severity::Warning;
                    d.confidence = Confidence::Medium;
                    out.push(d);
                }
            }
        }
        other if has_expression(other) => push_indeterminate(job_id, path, out),
        _ => {}
    }
}

fn push_indeterminate(job_id: &str, path: &str, out: &mut Vec<Diagnostic>) {
    let mut d = crate::finding(
        "self-hosted-runner",
        path,
        format!(
            "job `{job_id}`'s `runs-on:` is a runtime expression that may resolve to a self-hosted runner"
        ),
        "if this can expand to a self-hosted label, treat the job as untrusted-input-adjacent",
    );
    d.severity = Severity::Warning;
    d.confidence = Confidence::Medium;
    out.push(d);
}

fn self_hosted_runner_label_set(
    job_id: &str,
    path: &str,
    labels: &[String],
    out: &mut Vec<Diagnostic>,
) {
    if labels.iter().any(|l| l.eq_ignore_ascii_case("self-hosted")) {
        let mut d = crate::finding(
            "self-hosted-runner",
            path,
            format!("job `{job_id}` explicitly requests `self-hosted`"),
            "self-hosted runners can persist state between runs; prefer ephemeral runners for \
             public or untrusted workflows",
        );
        d.severity = Severity::Warning;
        d.confidence = Confidence::High;
        out.push(d);
        return;
    }
    if labels.iter().any(|l| l.contains("${{")) {
        push_indeterminate(job_id, path, out);
        return;
    }
    let unrecognized: Vec<&str> = labels
        .iter()
        .map(String::as_str)
        .filter(|l| !is_known_github_hosted_label(l) && !is_known_third_party_label(l))
        .collect();
    if !unrecognized.is_empty() {
        let names = unrecognized.join(", ");
        let mut d = crate::finding(
            "self-hosted-runner",
            path,
            format!(
                "job `{job_id}` requests label(s) [{names}] not recognized as GitHub-hosted or \
                 a known managed-runner provider; this may select a self-hosted runner"
            ),
            "confirm these labels resolve to GitHub-hosted or managed-ephemeral runners",
        );
        d.severity = Severity::Warning;
        d.confidence = Confidence::Medium;
        out.push(d);
    }
}

// ---------------------------------------------------------------------
// use-trusted-publishing
// ---------------------------------------------------------------------

const KNOWN_PYTHON_TP_INDICES: &[&str] = &[
    "https://upload.pypi.org/legacy/",
    "https://test.pypi.org/legacy/",
];
const KNOWN_RUBY_TP_INDICES: &[&str] = &["https://rubygems.org"];
const KNOWN_NPMJS_TP_INDICES: &[&str] =
    &["https://registry.npmjs.org", "https://registry.npmjs.org/"];

/// `None` (absent) counts as satisfied; `Some` must exactly match (after
/// trimming a trailing slash) one of `known`.
fn repo_url_ok(with: &serde_json::Map<String, Value>, key: &str, known: &[&str]) -> bool {
    match with.get(key) {
        None => true,
        Some(Value::String(s)) if !s.contains("${{") => known
            .iter()
            .any(|k| k.trim_end_matches('/') == s.trim_end_matches('/')),
        // Expression-valued or non-string: can't statically confirm it's a
        // known trusted-publishing index, so don't claim it is.
        _ => false,
    }
}

/// KNOWN LIMITATION: zizmor additionally scans `run:` script bodies (via a
/// tree-sitter bash/pwsh grammar) for ad hoc publish commands (`twine
/// upload`, `cargo publish`, `npm publish`, …) run without an `id-token`
/// permission. That requires a shell-script parser dependency this crate
/// doesn't have, and the brief steers away from "arbitrary script
/// substring guesses" — so this function only recognizes the four
/// well-known actions below by `uses:` + `with:` shape, matching zizmor's
/// `KNOWN_TRUSTED_PUBLISHING_ACTIONS` table.
fn use_trusted_publishing_step(step: &Value, path: &str, out: &mut Vec<Diagnostic>) {
    let Some(uses) = step.get("uses").and_then(Value::as_str) else {
        return;
    };
    let Some(Value::Object(with)) = step.get("with") else {
        return;
    };

    let finding = |key: &str, why: &str| {
        let mut d = crate::finding(
            "use-trusted-publishing",
            &child(&child(path, "with"), key),
            why,
            "prefer OIDC Trusted Publishing over a long-lived manually-configured credential",
        );
        d.severity = Severity::Info;
        d.confidence = Confidence::High;
        d
    };

    if uses_matches(uses, "pypa/gh-action-pypi-publish") {
        let has_password = with.contains_key("password");
        let repo_ok = repo_url_ok(with, "repository-url", KNOWN_PYTHON_TP_INDICES)
            && repo_url_ok(with, "repository_url", KNOWN_PYTHON_TP_INDICES);
        if has_password && repo_ok {
            out.push(finding(
                "password",
                "pypa/gh-action-pypi-publish uses a manually-configured `password` instead of \
                 Trusted Publishing",
            ));
        }
    } else if uses_matches(uses, "rubygems/release-gem") {
        if toolkit_bool(with.get("setup-trusted-publisher").unwrap_or(&Value::Null)) == Some(false)
        {
            out.push(finding(
                "setup-trusted-publisher",
                "rubygems/release-gem has `setup-trusted-publisher: false`",
            ));
        }
    } else if uses_matches(uses, "rubygems/configure-rubygems-credentials") {
        let has_token = with.contains_key("api-token");
        let server_ok = match with.get("gem-server") {
            None => true,
            Some(Value::String(s)) if !s.contains("${{") => KNOWN_RUBY_TP_INDICES
                .iter()
                .any(|k| *k == s.trim_end_matches('/')),
            _ => false,
        };
        if has_token && server_ok {
            out.push(finding(
                "api-token",
                "rubygems/configure-rubygems-credentials uses a manually-configured `api-token` \
                 instead of Trusted Publishing",
            ));
        }
    } else if uses_matches(uses, "actions/setup-node") {
        let registry_ok = repo_url_ok(with, "registry-url", KNOWN_NPMJS_TP_INDICES);
        let always_auth = with
            .get("always-auth")
            .and_then(toolkit_bool)
            .unwrap_or(false);
        if registry_ok && always_auth {
            out.push(finding(
                "always-auth",
                "actions/setup-node configures npmjs with `always-auth: true`, which implies a \
                 manually-configured credential instead of Trusted Publishing",
            ));
        }
    }
}

// ---------------------------------------------------------------------
// misfeature
// ---------------------------------------------------------------------

fn misfeature_step(step: &Value, path: &str, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    if let Some(uses) = step.get("uses").and_then(Value::as_str) {
        if uses_matches(uses, "actions/setup-python")
            && let Some(Value::Object(with)) = step.get("with")
            && with.contains_key("pip-install")
        {
            let mut d = crate::finding(
                "misfeature",
                &child(&child(path, "with"), "pip-install"),
                "actions/setup-python's `pip-install` installs packages into a global \
                 interpreter environment, which behaves inconsistently across runners",
                "always use a virtual environment to manage Python packages",
            );
            d.severity = Severity::Info;
            d.confidence = Confidence::High;
            out.push(d);
        }
        return;
    }

    if step.get("run").and_then(Value::as_str).is_none() {
        return;
    }
    let shell = step
        .get("shell")
        .and_then(Value::as_str)
        .map(normalize_shell);
    match shell.as_deref() {
        Some("bash" | "pwsh" | "python" | "sh" | "powershell") | None => {}
        Some("cmd") => {
            let mut d = crate::finding(
                "misfeature",
                &child(path, "shell"),
                "`shell: cmd` has no formal grammar and hasn't been the Windows default since \
                 2019, which limits static analysis of this step",
                "use `shell: pwsh` or `shell: bash` instead",
            );
            d.severity = Severity::Info;
            d.confidence = Confidence::High;
            out.push(d);
        }
        Some(other) => {
            if !persona_enabled(ctx, Persona::Auditor) {
                return;
            }
            let mut d = crate::finding(
                "misfeature",
                &child(path, "shell"),
                format!("`shell: {other}` is not one of GitHub Actions' well-known shells"),
                "use a well-known shell, like `bash` or `pwsh`, for improved analysis",
            );
            d.severity = Severity::Info;
            d.confidence = Confidence::High;
            out.push(d);
        }
    }
}

/// Lowercased basename without a trailing `.exe`, e.g. `C:\Program
/// Files\PowerShell\pwsh.EXE` → `pwsh`.
fn normalize_shell(shell: &str) -> String {
    shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shell)
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE")
        .to_ascii_lowercase()
}

// ---------------------------------------------------------------------
// superfluous-actions
// ---------------------------------------------------------------------

const SUPERFLUOUS_ACTIONS: &[(&str, &str, Persona, Confidence)] = &[
    (
        "ncipollo/release-action",
        "use `gh release` in a script step",
        Persona::Regular,
        Confidence::High,
    ),
    (
        "softprops/action-gh-release",
        "use `gh release` in a script step",
        Persona::Regular,
        Confidence::High,
    ),
    (
        "elgohr/Github-Release-Action",
        "use `gh release` in a script step",
        Persona::Regular,
        Confidence::High,
    ),
    (
        "peter-evans/create-pull-request",
        "use `gh pr create` in a script step",
        // Pedantic: replicating full PR creation with just `gh`/`git` is
        // comparatively cumbersome.
        Persona::Pedantic,
        Confidence::Medium,
    ),
    (
        "peter-evans/create-or-update-comment",
        "use `gh pr comment` or `gh issue comment` in a script step",
        // Pedantic: `gh` has no edit-comment-by-id equivalent.
        Persona::Pedantic,
        Confidence::Medium,
    ),
    (
        "dacbd/create-issue-action",
        "use `gh issue create` in a script step",
        Persona::Regular,
        Confidence::High,
    ),
    (
        "actions-ecosystem/action-add-labels",
        "use `gh issue edit --add-label` or `gh pr edit --add-label` in a script step",
        Persona::Regular,
        Confidence::High,
    ),
    (
        "actions-ecosystem/action-remove-labels",
        "use `gh issue edit --remove-label` or `gh pr edit --remove-label` in a script step",
        Persona::Regular,
        Confidence::High,
    ),
    (
        "svenstaro/upload-release-action",
        "use `gh release create` and `gh release upload` in a script step",
        Persona::Regular,
        Confidence::High,
    ),
    (
        "addnab/docker-run-action",
        "use `docker run` in a script step, or use a container step",
        Persona::Regular,
        Confidence::High,
    ),
    (
        "sergeysova/jq-action",
        "use `jq` in a script step",
        Persona::Regular,
        Confidence::High,
    ),
    (
        "dtolnay/rust-toolchain",
        "use `rustup` and/or `cargo` in a script step",
        // Pedantic: this action also does extra environment setup users
        // may rely on; the equivalence isn't exact.
        Persona::Pedantic,
        Confidence::Medium,
    ),
    (
        "stefanzweifel/git-auto-commit-action",
        "use `git add`, `git commit`, and `git push` in a script step",
        // Pedantic: replicating empty-commit detection and auth setup
        // takes real care.
        Persona::Pedantic,
        Confidence::Medium,
    ),
    (
        "EndBug/add-and-commit",
        "use `git add`, `git commit`, and `git push` in a script step",
        Persona::Pedantic,
        Confidence::Medium,
    ),
];

fn superfluous_actions_step(
    step: &Value,
    path: &str,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
) {
    let Some(uses) = step.get("uses").and_then(Value::as_str) else {
        return;
    };
    for (owner_repo, tip, persona, confidence) in SUPERFLUOUS_ACTIONS {
        if uses_matches(uses, owner_repo) && persona_enabled(ctx, *persona) {
            let mut d = crate::finding(
                "superfluous-actions",
                &child(path, "uses"),
                format!("`{owner_repo}`'s functionality is already available on the runner"),
                tip,
            );
            d.severity = Severity::Info;
            d.confidence = *confidence;
            out.push(d);
        }
    }
}

// ---------------------------------------------------------------------
// github-app
// ---------------------------------------------------------------------

/// Permissions that only affect organization-level resources, not
/// repository contents. When `create-github-app-token`'s requested
/// `permission-*` set is a (non-empty) subset of this, omitting
/// `repositories:` alongside `owner:` isn't excessive — the token can't
/// touch repository resources regardless.
const ORG_ONLY_PERMISSIONS: &[&str] = &[
    "permission-custom-properties-for-organizations",
    "permission-enterprise-custom-properties-for-organizations",
    "permission-members",
    "permission-organization-administration",
    "permission-organization-announcement-banners",
    "permission-organization-copilot-seat-management",
    "permission-organization-custom-org-roles",
    "permission-organization-custom-properties",
    "permission-organization-custom-roles",
    "permission-organization-events",
    "permission-organization-hooks",
    "permission-organization-packages",
    "permission-organization-personal-access-token-requests",
    "permission-organization-personal-access-tokens",
    "permission-organization-plan",
    "permission-organization-projects",
    "permission-organization-secrets",
    "permission-organization-self-hosted-runners",
    "permission-organization-user-blocking",
];

fn github_app_step(step: &Value, path: &str, out: &mut Vec<Diagnostic>) {
    let Some(uses) = step.get("uses").and_then(Value::as_str) else {
        return;
    };
    if !uses_matches(uses, "actions/create-github-app-token") {
        return;
    }
    let Some(Value::Object(with)) = step.get("with") else {
        // `with:` entirely absent still means "no `permission-*` set",
        // which is the blanket-permissions case below.
        github_app_missing_permissions(path, out);
        return;
    };
    let with_path = child(path, "with");

    if let Some(v) = with.get("skip-token-revoke") {
        match toolkit_bool(v) {
            Some(true) => {
                let mut d = crate::finding(
                    "github-app",
                    &child(&with_path, "skip-token-revoke"),
                    "actions/create-github-app-token has `skip-token-revoke: true`, disabling \
                     the action's default token-revocation step",
                    "remove `skip-token-revoke` so the generated token is revoked at the end of \
                     the job",
                );
                d.severity = Severity::Error;
                d.confidence = Confidence::High;
                out.push(d);
            }
            Some(false) => {}
            None if has_expression(v) => {
                let mut d = crate::finding(
                    "github-app",
                    &child(&with_path, "skip-token-revoke"),
                    "actions/create-github-app-token's `skip-token-revoke` depends on a runtime \
                     expression and may evaluate to `true`",
                    "pin `skip-token-revoke` to a literal `false`, or remove it",
                );
                d.severity = Severity::Error;
                d.confidence = Confidence::Medium;
                out.push(d);
            }
            None => {}
        }
    }

    let permissions: BTreeSet<&str> = with
        .keys()
        .map(String::as_str)
        .filter(|k| k.starts_with("permission-"))
        .collect();

    if with.contains_key("owner")
        && !with.contains_key("repositories")
        && !(!permissions.is_empty()
            && permissions.iter().all(|p| ORG_ONLY_PERMISSIONS.contains(p)))
    {
        let mut d = crate::finding(
            "github-app",
            &child(&with_path, "owner"),
            "actions/create-github-app-token sets `owner:` without `repositories:`, granting \
             the token access to every repository the app is installed on",
            "set `repositories: 'repo1,repo2'` to scope the token to specific repositories",
        );
        d.severity = Severity::Error;
        d.confidence = Confidence::High;
        out.push(d);
    }

    if permissions.is_empty() {
        github_app_missing_permissions(path, out);
    }
}

fn github_app_missing_permissions(path: &str, out: &mut Vec<Diagnostic>) {
    let mut d = crate::finding(
        "github-app",
        &child(path, "uses"),
        "actions/create-github-app-token has no `permission-<name>` input, so the generated \
         token inherits the app's full installation permissions",
        "set at least one `permission-<name>: read|write` input to scope the token",
    );
    d.severity = Severity::Error;
    d.confidence = Confidence::High;
    out.push(d);
}

// ---------------------------------------------------------------------
// dependabot-execution / dependabot-cooldown
//
// NOTE: `doc["updates"]` is never populated through the current `audit()`
// entrypoint (confirmed with SecCore: `WorkflowDocument` is strictly
// workflow-shaped). These two functions are implemented and tested
// directly against synthetic Dependabot-shaped `Value`s; they are
// permanently unreachable through `check()`/`audit()` until a Dependabot
// config entrypoint is added upstream. See crate-level report to Main.
// ---------------------------------------------------------------------

fn dependabot_execution(doc: &Value, out: &mut Vec<Diagnostic>) {
    let Some(updates) = doc.get("updates").and_then(Value::as_array) else {
        return;
    };
    for (idx, update) in updates.iter().enumerate() {
        let allow = update
            .get("insecure-external-code-execution")
            .and_then(Value::as_str)
            == Some("allow");
        if allow {
            let path = format!("/updates/{idx}/insecure-external-code-execution");
            let mut d = crate::finding(
                "dependabot-execution",
                &path,
                "this Dependabot update allows third-party pre/post-update scripts to execute \
                 (`insecure-external-code-execution: allow`)",
                "set `insecure-external-code-execution: deny` unless you specifically vetted \
                 and need this ecosystem's update scripts",
            );
            d.severity = Severity::Error;
            d.confidence = Confidence::High;
            out.push(d);
        }
    }
}

/// `AuditContext` carries no cooldown-policy override, so this uses
/// zizmor's own built-in default minimum (7 days;
/// `DependabotCooldownConfig::default()`), which is also Dependabot's own
/// documented implicit `cooldown.default-days` when `cooldown:` is unset
/// or doesn't specify it.
fn dependabot_cooldown(doc: &Value, out: &mut Vec<Diagnostic>) {
    const MINIMUM_DAYS: u64 = 7;
    let Some(updates) = doc.get("updates").and_then(Value::as_array) else {
        return;
    };
    for (idx, update) in updates.iter().enumerate() {
        let cooldown = update.get("cooldown");
        let default_days = match cooldown {
            Some(Value::Object(c)) => c.get("default-days").and_then(Value::as_u64).unwrap_or(3),
            Some(Value::Null) | None => 3,
            Some(_) => 3,
        };
        if default_days >= MINIMUM_DAYS {
            continue;
        }
        let path = match cooldown {
            Some(Value::Object(c)) if c.contains_key("default-days") => {
                format!("/updates/{idx}/cooldown/default-days")
            }
            Some(Value::Object(_)) => format!("/updates/{idx}/cooldown"),
            _ => format!("/updates/{idx}"),
        };
        let mut d = crate::finding(
            "dependabot-cooldown",
            &path,
            format!(
                "this Dependabot update's cooldown ({default_days} day(s)) is shorter than the \
                 recommended minimum ({MINIMUM_DAYS} days), giving less time for a malicious or \
                 broken release to be caught before it's pulled in"
            ),
            "set `cooldown.default-days` to at least the recommended minimum",
        );
        d.severity = Severity::Warning;
        d.confidence = Confidence::High;
        out.push(d);
    }
}

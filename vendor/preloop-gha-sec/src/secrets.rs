//! Secret-handling audits: credential persistence, hardcoded registry
//! passwords, insecure workflow commands, blanket secret inheritance,
//! whole-context secret exposure, redaction bypasses, and secrets used
//! outside a dedicated environment.
//!
//! These checks are native reimplementations against this workspace's own
//! `serde_json::Value` workflow tree and `preloop-gha-expressions` parser.
//! Their scope and intent are informed by (but not copied from) zizmor's
//! MIT-licensed `artipacked`, `hardcoded-container-credentials`,
//! `insecure-commands`, `secrets-inherit`, `overprovisioned-secrets`,
//! `unredacted-secrets` and `secrets-outside-env` audits
//! (<https://github.com/zizmorcore/zizmor>, MIT License). Rule IDs are kept
//! identical to zizmor's for operator familiarity; the detection logic below
//! is written independently against this crate's APIs.
//!
//! ## `if: false` policy
//!
//! Checks about *runtime* behavior (credential persistence, insecure
//! commands, secret inheritance, expression evaluation) skip jobs/steps with
//! a literal `if: false` (or `if: ${{ false }}`), since that code never
//! executes. `hardcoded_container_credentials` is the one exception: a
//! hardcoded password is a source-control hygiene problem regardless of
//! whether the job ever runs, so it is reported even under a disabled job.

use std::collections::BTreeSet;

use preloop_gha_expressions::collect_secret_reads;
use serde_json::Value;

use crate::{AuditContext, Confidence, Diagnostic, Persona, Severity};

pub(crate) fn check(doc: &Value, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    let expressions = collect_expressions(doc);

    artipacked(doc, ctx, out);
    hardcoded_container_credentials(doc, out);
    insecure_commands(doc, ctx, out);
    secrets_inherit(doc, out);
    overprovisioned_secrets(&expressions, out);
    unredacted_secrets(&expressions, out);
    secrets_outside_env(doc, ctx, &expressions, out);
}

// ---------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------

/// Rank used to decide whether `ctx`'s selected persona enables a finding
/// gated at `required`. Auditor sees everything; Regular sees only findings
/// that don't require `required` above Regular.
fn persona_rank(persona: Persona) -> u8 {
    match persona {
        Persona::Regular => 0,
        Persona::Pedantic => 1,
        Persona::Auditor => 2,
    }
}

fn persona_enabled(ctx: Persona, required: Persona) -> bool {
    persona_rank(ctx) >= persona_rank(required)
}

/// A literal `if: false` / `if: 'false'` / `if: ${{ false }}`. Anything more
/// complex (variable, comparison, function call) is left alone: we don't
/// evaluate conditions, and a wrongly-assumed "disabled" would hide a real
/// finding, which is worse than an occasional extra one.
fn is_disabled(obj: &Value) -> bool {
    let Some(if_value) = obj.get("if") else {
        return false;
    };
    match if_value {
        Value::Bool(b) => !*b,
        Value::String(s) => {
            let t = s.trim();
            if t.eq_ignore_ascii_case("false") {
                return true;
            }
            if let Some(inner) = t.strip_prefix("${{").and_then(|r| r.strip_suffix("}}")) {
                return inner.trim().eq_ignore_ascii_case("false");
            }
            false
        }
        _ => false,
    }
}

fn is_expression(s: &str) -> bool {
    let t = s.trim();
    t.starts_with("${{") && t.ends_with("}}") && t.len() >= 6
}

/// GitHub Actions' `bool.TryParse`-flavored coercion: only a boolean `true`
/// or a string that trims to `"true"` (case-insensitive) is truthy.
fn csharp_bool(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::String(s) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// A two-/three-state read of a YAML boolean-ish field: concrete `true`,
/// concrete `false`, or "not a literal boolean" (expression or garbage).
enum BoolField {
    True,
    False,
    NotLiteral,
}

fn bool_field(value: &Value) -> BoolField {
    match value {
        Value::Bool(true) => BoolField::True,
        Value::Bool(false) => BoolField::False,
        Value::String(s) => {
            let t = s.trim();
            if t.eq_ignore_ascii_case("true") {
                BoolField::True
            } else if t.eq_ignore_ascii_case("false") {
                BoolField::False
            } else {
                BoolField::NotLiteral
            }
        }
        _ => BoolField::NotLiteral,
    }
}

/// Every normal job in the document, as `(job_id, job_pointer, job_value)`,
/// skipping reusable-workflow-call jobs (`uses` at job level) and disabled
/// jobs (`if: false`). `include_disabled` lets `hardcoded_container_credentials`
/// opt out of the disabled-job skip.
fn normal_jobs(doc: &Value, include_disabled: bool) -> Vec<(String, String, &Value)> {
    let mut jobs = Vec::new();
    let Some(Value::Object(job_map)) = doc.get("jobs") else {
        return jobs;
    };
    for (job_id, job) in job_map {
        if job.get("uses").is_some() {
            continue;
        }
        if !include_disabled && is_disabled(job) {
            continue;
        }
        jobs.push((job_id.clone(), crate::pointer("/jobs", job_id), job));
    }
    jobs
}

/// All jobs (normal or reusable-call), honoring the disabled-skip.
fn all_jobs(doc: &Value) -> Vec<(String, String, &Value)> {
    let mut jobs = Vec::new();
    let Some(Value::Object(job_map)) = doc.get("jobs") else {
        return jobs;
    };
    for (job_id, job) in job_map {
        if is_disabled(job) {
            continue;
        }
        jobs.push((job_id.clone(), crate::pointer("/jobs", job_id), job));
    }
    jobs
}

/// Non-disabled steps of a normal job, as `(index, step_pointer, step_value)`.
fn steps_of<'a>(job_ptr: &str, job: &'a Value) -> Vec<(usize, String, &'a Value)> {
    let mut steps = Vec::new();
    let Some(Value::Array(array)) = job.get("steps") else {
        return steps;
    };
    let steps_ptr = crate::pointer(job_ptr, "steps");
    for (i, step) in array.iter().enumerate() {
        if is_disabled(step) {
            continue;
        }
        steps.push((i, crate::pointer(&steps_ptr, &i.to_string()), step));
    }
    steps
}

/// Does the workflow declare `on: workflow_call` (in any of its three
/// shapes: bare string, list, or map)?
fn workflow_has_workflow_call(doc: &Value) -> bool {
    match doc.get("on") {
        Some(Value::String(s)) => s == "workflow_call",
        Some(Value::Array(items)) => items.iter().any(|v| v.as_str() == Some("workflow_call")),
        Some(Value::Object(map)) => map.contains_key("workflow_call"),
        _ => false,
    }
}

// ---------------------------------------------------------------------
// Expression extraction: find every `${{ ... }}` in the document, skipping
// disabled jobs/steps, and remember where each one was found.
// ---------------------------------------------------------------------

struct FoundExpr {
    pointer: String,
    /// Bare expression text, `${{` / `}}` stripped, trimmed.
    expr: String,
}

fn collect_expressions(doc: &Value) -> Vec<FoundExpr> {
    let mut out = Vec::new();
    let Value::Object(root) = doc else {
        return out;
    };
    for (key, value) in root {
        if key == "jobs" {
            continue;
        }
        walk_strings(value, &crate::pointer("", key), &mut out);
    }
    for (_, job_ptr, job) in all_jobs(doc) {
        let Value::Object(job_map) = job else {
            continue;
        };
        for (key, value) in job_map {
            if key == "steps" {
                continue;
            }
            walk_strings(value, &crate::pointer(&job_ptr, key), &mut out);
        }
        for (_, step_ptr, step) in steps_of(&job_ptr, job) {
            walk_strings(step, &step_ptr, &mut out);
        }
    }
    out
}

fn walk_strings(value: &Value, pointer: &str, out: &mut Vec<FoundExpr>) {
    match value {
        Value::String(s) => {
            for frag in extract_fenced_expressions(s) {
                out.push(FoundExpr {
                    pointer: pointer.to_string(),
                    expr: frag.to_string(),
                });
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk_strings(item, &crate::pointer(pointer, &i.to_string()), out);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                walk_strings(item, &crate::pointer(pointer, key), out);
            }
        }
        _ => {}
    }
}

/// Find every `${{ ... }}` fenced region in `s`, respecting single-quoted
/// string literals (with `''` escaping) so a literal `}}` inside a quoted
/// argument doesn't end the fence early. Operates on bytes only; every slice
/// boundary used is adjacent to a single-byte ASCII delimiter, so this is
/// safe on arbitrary UTF-8 input.
fn extract_fenced_expressions(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = s[search_from..].find("${{") {
        let start = search_from + rel + 3;
        let bytes = s.as_bytes();
        let n = bytes.len();
        let mut j = start;
        let mut in_quote = false;
        let mut end = None;
        while j < n {
            let c = bytes[j];
            if in_quote {
                if c == b'\'' {
                    if j + 1 < n && bytes[j + 1] == b'\'' {
                        j += 2;
                        continue;
                    }
                    in_quote = false;
                }
                j += 1;
                continue;
            }
            match c {
                b'\'' => {
                    in_quote = true;
                    j += 1;
                }
                b'}' if j + 1 < n && bytes[j + 1] == b'}' => {
                    end = Some(j);
                    break;
                }
                _ => j += 1,
            }
        }
        match end {
            Some(e) => {
                out.push(s[start..e].trim());
                search_from = e + 2;
            }
            None => break,
        }
    }
    out
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Find the index of the `)` matching the `(` at `bytes[open]`, respecting
/// nested parens and single-quoted string literals.
fn matching_paren(bytes: &[u8], open: usize) -> Option<usize> {
    let n = bytes.len();
    let mut depth = 1usize;
    let mut in_quote = false;
    let mut j = open + 1;
    while j < n {
        let c = bytes[j];
        if in_quote {
            if c == b'\'' {
                if j + 1 < n && bytes[j + 1] == b'\'' {
                    j += 2;
                    continue;
                }
                in_quote = false;
            }
            j += 1;
            continue;
        }
        match c {
            b'\'' => {
                in_quote = true;
                j += 1;
            }
            b'(' => {
                depth += 1;
                j += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
                j += 1;
            }
            _ => j += 1,
        }
    }
    None
}

/// Find every top-level-or-nested call to the (case-insensitive) function
/// `name` within `expr`, returning each call's single-argument text
/// (trimmed). `fromJSON`/`toJSON` both take exactly one argument in GitHub
/// Actions expression syntax, so no top-level-comma splitting is needed.
fn find_calls<'a>(expr: &'a str, name: &str) -> Vec<&'a str> {
    let needle = name.as_bytes();
    let bytes = expr.as_bytes();
    let n = bytes.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let matches = i + needle.len() <= n
            && needle
                .iter()
                .enumerate()
                .all(|(k, &nb)| bytes[i + k].to_ascii_lowercase() == nb);
        if matches {
            let prev_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after = i + needle.len();
            let next_ok = after >= n || !is_ident_byte(bytes[after]);
            if prev_ok
                && next_ok
                && let mut k = after
                && {
                    while k < n && bytes[k].is_ascii_whitespace() {
                        k += 1;
                    }
                    k < n && bytes[k] == b'('
                }
                && let Some(close) = matching_paren(bytes, k)
            {
                out.push(expr[k + 1..close].trim());
                i = close + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------
// artipacked
// ---------------------------------------------------------------------

/// Parse a leading `v`/`V`-optional major version number out of a checkout
/// ref (`v6`, `6.0.1`, `6`). Returns `None` for anything that doesn't start
/// with digits, including commit SHAs (which we can't resolve to a tag
/// without network access).
fn checkout_major_version(git_ref: &str) -> Option<u64> {
    let t = git_ref.trim();
    let looks_like_sha = t.len() >= 7 && t.len() <= 40 && t.chars().all(|c| c.is_ascii_hexdigit());
    if looks_like_sha {
        return None;
    }
    let stripped = t
        .strip_prefix('v')
        .or_else(|| t.strip_prefix('V'))
        .unwrap_or(t);
    let digits: String = stripped
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

fn uses_repo_slug(uses: &str) -> Option<(&str, &str)> {
    let path = uses.split('@').next().unwrap_or(uses);
    let ref_part = uses.rsplit('@').next().unwrap_or("");
    if path == uses {
        // no '@' at all
        return None;
    }
    Some((path, ref_part))
}

fn matches_action(path: &str, owner_repo: &str) -> bool {
    let mut parts = path.splitn(3, '/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    let slug = format!("{owner}/{repo}");
    slug.eq_ignore_ascii_case(owner_repo)
}

/// Severity for `v6.0.0`+ `actions/checkout` is downgraded: the persisted
/// credential moved from `.git/config` (readable by anything that reads the
/// checkout) to `$RUNNER_TEMP` (a more misuse-resistant location).
fn artipacked_severity(major: Option<u64>, has_upload: bool) -> Severity {
    let v6_or_higher = major.is_some_and(|m| m >= 6);
    match (v6_or_higher, has_upload) {
        (true, false) => Severity::Info,
        (true, true) => Severity::Warning,
        (false, false) => Severity::Warning,
        (false, true) => Severity::Error,
    }
}

fn dangerous_artifact_patterns(path: &str) -> bool {
    for line in path.lines() {
        let p = line.trim();
        if p.is_empty() || p.starts_with('#') {
            continue;
        }
        match p {
            "." | "./" | ".." | "../" => return true,
            _ => {
                if is_expression(p) {
                    let inner = &p[3..p.len() - 2];
                    if inner.to_ascii_lowercase().contains("github.workspace") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

struct VulnerableCheckout {
    index: usize,
    pointer: String,
    major: Option<u64>,
    persona: Persona,
}

fn artipacked(doc: &Value, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    for (_, job_ptr, job) in normal_jobs(doc, false) {
        let mut checkouts: Vec<VulnerableCheckout> = Vec::new();
        let mut uploads: Vec<(usize, String)> = Vec::new();

        for (index, step_ptr, step) in steps_of(&job_ptr, job) {
            let Some(uses) = step.get("uses").and_then(Value::as_str) else {
                continue;
            };
            let Some((path, git_ref)) = uses_repo_slug(uses) else {
                continue;
            };

            if matches_action(path, "actions/checkout") {
                let with = step.get("with");
                // Whole `with:` block is itself an expression: can't inspect.
                if matches!(with, Some(Value::String(_))) {
                    continue;
                }
                let persist = with.and_then(|w| w.get("persist-credentials"));
                let persona = match persist {
                    None => Persona::Regular,
                    Some(v) => match bool_field(v) {
                        BoolField::False => continue, // explicitly safe
                        BoolField::True => Persona::Auditor,
                        BoolField::NotLiteral => {
                            if v.as_str().is_some_and(is_expression) {
                                Persona::Pedantic
                            } else {
                                Persona::Regular
                            }
                        }
                    },
                };
                checkouts.push(VulnerableCheckout {
                    index,
                    pointer: step_ptr,
                    major: checkout_major_version(git_ref),
                    persona,
                });
            } else if matches_action(path, "actions/upload-artifact") {
                let Some(Value::String(upload_path)) = step.get("with").and_then(|w| w.get("path"))
                else {
                    continue;
                };
                if dangerous_artifact_patterns(upload_path) {
                    uploads.push((index, step_ptr));
                }
            }
        }

        for checkout in &checkouts {
            if !persona_enabled(ctx.persona, checkout.persona) {
                continue;
            }
            let paired = uploads.iter().find(|(i, _)| *i > checkout.index);
            let has_upload = paired.is_some();
            let mut d = crate::finding(
                "artipacked",
                &checkout.pointer,
                "actions/checkout does not set persist-credentials: false",
                "set `persist-credentials: false` unless the credential is needed, in which \
                 case set `persist-credentials: true` explicitly",
            );
            d.severity = artipacked_severity(checkout.major, has_upload);
            d.confidence = if has_upload {
                Confidence::High
            } else {
                Confidence::Medium
            };
            if let Some((_, upload_ptr)) = paired {
                d.message = format!(
                    "{} (persisted credential may leak via upload at {upload_ptr})",
                    d.message
                );
            }
            out.push(d);
        }
    }
}

// ---------------------------------------------------------------------
// hardcoded-container-credentials
// ---------------------------------------------------------------------

fn check_container_password(container_ptr: &str, container: &Value, out: &mut Vec<Diagnostic>) {
    let Some(Value::String(password)) =
        container.get("credentials").and_then(|c| c.get("password"))
    else {
        return;
    };
    if is_expression(password) {
        return;
    }
    let mut d = crate::finding(
        "hardcoded-container-credentials",
        &crate::pointer(container_ptr, "credentials"),
        "container registry password is hard-coded instead of using a secret",
        "replace the literal password with `${{ secrets.<NAME> }}` and store the value as an \
         encrypted secret",
    );
    d.severity = Severity::Error;
    d.confidence = Confidence::High;
    out.push(d);
}

fn hardcoded_container_credentials(doc: &Value, out: &mut Vec<Diagnostic>) {
    // Static source-hygiene concern: a disabled job still has the secret
    // committed in plaintext, so unlike the runtime checks below we do not
    // skip `if: false` jobs here.
    for (_, job_ptr, job) in normal_jobs(doc, true) {
        if let Some(container) = job.get("container")
            && container.is_object()
        {
            check_container_password(&crate::pointer(&job_ptr, "container"), container, out);
        }
        if let Some(Value::Object(services)) = job.get("services") {
            let services_ptr = crate::pointer(&job_ptr, "services");
            for (name, service) in services {
                if service.is_object() {
                    check_container_password(&crate::pointer(&services_ptr, name), service, out);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// insecure-commands
// ---------------------------------------------------------------------

fn check_env_block(
    env_value: Option<&Value>,
    pointer: &str,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
) {
    match env_value {
        None => {}
        Some(Value::Object(env)) => {
            let enabled = env
                .get("ACTIONS_ALLOW_UNSECURE_COMMANDS")
                .is_some_and(csharp_bool);
            if enabled {
                let mut d = crate::finding(
                    "insecure-commands",
                    &crate::pointer(pointer, "env"),
                    "ACTIONS_ALLOW_UNSECURE_COMMANDS re-enables deprecated, injectable workflow \
                     commands",
                    "remove `ACTIONS_ALLOW_UNSECURE_COMMANDS` and use `GITHUB_PATH` / \
                     `GITHUB_OUTPUT` / `GITHUB_ENV` instead of `::set-env`/`::add-path`",
                );
                d.severity = Severity::Error;
                d.confidence = Confidence::High;
                out.push(d);
            }
        }
        // Whole `env:` block is an expression; we can't see into it.
        Some(_) if persona_enabled(ctx.persona, Persona::Auditor) => {
            let mut d = crate::finding(
                "insecure-commands",
                &crate::pointer(pointer, "env"),
                "non-static environment may set ACTIONS_ALLOW_UNSECURE_COMMANDS",
                "inline the environment block statically so it can be audited, or confirm it \
                 never sets ACTIONS_ALLOW_UNSECURE_COMMANDS",
            );
            d.severity = Severity::Error;
            d.confidence = Confidence::Medium;
            out.push(d);
        }
        Some(_) => {}
    }
}

fn insecure_commands(doc: &Value, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    check_env_block(doc.get("env"), "", ctx, out);

    for (_, job_ptr, job) in normal_jobs(doc, false) {
        check_env_block(job.get("env"), &job_ptr, ctx, out);

        for (_, step_ptr, step) in steps_of(&job_ptr, job) {
            // Workflow commands come from scripts, so only `run:` steps
            // matter here (a `uses:` step's own env handling is the called
            // action's business, out of scope for this document).
            if step.get("run").is_none() {
                continue;
            }
            check_env_block(step.get("env"), &step_ptr, ctx, out);
        }
    }
}

// ---------------------------------------------------------------------
// secrets-inherit
// ---------------------------------------------------------------------

fn secrets_inherit(doc: &Value, out: &mut Vec<Diagnostic>) {
    for (_, job_ptr, job) in all_jobs(doc) {
        if job.get("uses").is_none() {
            continue;
        }
        let is_inherit = matches!(job.get("secrets"), Some(Value::String(s)) if s.eq_ignore_ascii_case("inherit"));
        if !is_inherit {
            continue;
        }
        let mut d = crate::finding(
            "secrets-inherit",
            &crate::pointer(&job_ptr, "secrets"),
            "reusable workflow call unconditionally inherits every parent secret",
            "replace `secrets: inherit` with an explicit `secrets:` map that forwards only the \
             secrets the called workflow actually needs",
        );
        d.severity = Severity::Warning;
        d.confidence = Confidence::High;
        out.push(d);
    }
}

// ---------------------------------------------------------------------
// overprovisioned-secrets
// ---------------------------------------------------------------------

fn overprovisioned_secrets(expressions: &[FoundExpr], out: &mut Vec<Diagnostic>) {
    for found in expressions {
        // High-confidence, narrow pattern: `toJSON(secrets)` on the bare
        // whole-object context, matching the common real-world mistake.
        let bare_tojson = find_calls(&found.expr, "tojson")
            .into_iter()
            .any(|arg| arg.trim().eq_ignore_ascii_case("secrets"));
        if bare_tojson {
            let mut d = crate::finding(
                "overprovisioned-secrets",
                &found.pointer,
                "toJSON(secrets) injects the entire secrets context into the runner",
                "access secrets individually by name (`secrets.NAME`) instead of serializing the \
                 whole `secrets` context",
            );
            d.severity = Severity::Warning;
            d.confidence = Confidence::High;
            out.push(d);
            continue;
        }

        // Broader, lower-confidence bucket: any other way of referencing
        // the whole/dynamic secrets context (bare `secrets`, `secrets[...]`
        // with a computed key, or `secrets.*`) that we can't statically
        // prove is scoped to one name.
        if let Ok(reads) = collect_secret_reads(&found.expr)
            && reads.dynamic
        {
            let mut d = crate::finding(
                "overprovisioned-secrets",
                &found.pointer,
                "expression may expose the entire secrets context (dynamic or whole-object \
                 access)",
                "access secrets individually by literal name; avoid computed indices and \
                 whole-object references",
            );
            d.severity = Severity::Warning;
            d.confidence = Confidence::Medium;
            out.push(d);
        }
    }
}

// ---------------------------------------------------------------------
// unredacted-secrets
// ---------------------------------------------------------------------

fn unredacted_secrets(expressions: &[FoundExpr], out: &mut Vec<Diagnostic>) {
    for found in expressions {
        for arg in find_calls(&found.expr, "fromjson") {
            let Ok(reads) = collect_secret_reads(arg) else {
                continue;
            };
            let is_bare_secrets = arg.trim().eq_ignore_ascii_case("secrets");
            let is_dynamic_child = reads.dynamic && !is_bare_secrets && reads.names.is_empty();
            let is_child_of_secrets = !reads.names.is_empty() || is_dynamic_child;
            if !is_child_of_secrets {
                continue;
            }
            let mut d = if is_dynamic_child {
                crate::finding(
                    "unredacted-secrets",
                    &found.pointer,
                    "fromJSON() may bypass GitHub's log redaction for a dynamically selected secret",
                    "resolve the secret name statically; store structured secret fields as separate \
                     individual secrets instead of JSON-decoding a dynamically selected secret",
                )
            } else {
                crate::finding(
                    "unredacted-secrets",
                    &found.pointer,
                    "fromJSON() on a secret value bypasses GitHub's log redaction",
                    "store structured secret fields as separate individual secrets instead of \
                     JSON-decoding a secret at runtime",
                )
            };
            d.severity = Severity::Warning;
            d.confidence = if is_dynamic_child {
                Confidence::Medium
            } else {
                Confidence::High
            };
            out.push(d);
        }
    }
}

// ---------------------------------------------------------------------
// secrets-outside-env
// ---------------------------------------------------------------------

fn secrets_outside_env(
    doc: &Value,
    ctx: &AuditContext,
    expressions: &[FoundExpr],
    out: &mut Vec<Diagnostic>,
) {
    // As of zizmor 1.24.0 these findings are auditor-only: fixing them
    // requires judgment about environment/reusable-workflow interactions
    // that GitHub's platform doesn't document cleanly.
    if !persona_enabled(ctx.persona, Persona::Auditor) {
        return;
    }

    // A workflow that is itself callable (`on: workflow_call`) can't rely
    // on environment-scoped secrets unless the caller passes
    // `secrets: inherit` (already flagged by `secrets-inherit`), so we
    // don't have a reliable baseline here.
    if workflow_has_workflow_call(doc) {
        return;
    }

    for (_, job_ptr, job) in normal_jobs(doc, false) {
        if job.get("environment").is_some() {
            continue;
        }
        let job_prefix = format!("{job_ptr}/");

        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut dynamic = false;
        for found in expressions {
            if found.pointer != job_ptr && !found.pointer.starts_with(&job_prefix) {
                continue;
            }
            let Ok(reads) = collect_secret_reads(&found.expr) else {
                continue;
            };
            for name in reads.names {
                if !name.eq_ignore_ascii_case("GITHUB_TOKEN") {
                    names.insert(name);
                }
            }
            if reads.dynamic {
                dynamic = true;
            }
        }

        for name in &names {
            let mut d = crate::finding(
                "secrets-outside-env",
                &job_ptr,
                format!("secret `{name}` is accessed outside of a dedicated environment"),
                "configure this secret on a deployment environment and add `environment: <name>` \
                 to the job so the secret is scoped by the environment's protection rules",
            );
            d.severity = Severity::Warning;
            d.confidence = Confidence::High;
            out.push(d);
        }
        if dynamic {
            let mut d = crate::finding(
                "secrets-outside-env",
                &job_ptr,
                "secrets context is accessed dynamically outside of a dedicated environment",
                "resolve which secret is actually needed and scope it to a deployment \
                 environment",
            );
            d.severity = Severity::Warning;
            d.confidence = Confidence::Medium;
            out.push(d);
        }
    }
}

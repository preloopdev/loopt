//! Trust-boundary audits: attacker-controlled data reaching code-execution
//! sinks, dangerous triggers, spoofable actor checks, cache poisoning, and
//! GitHub Actions expression footguns that silently defeat `if:` gates.
//!
//! `preloop-gha-expressions` does not expose an expression AST publicly (only
//! context collection, evaluation and truthiness helpers are `pub`), so the
//! condition/ternary/`contains()` analyses below use small, local,
//! quote-and-paren-aware text splitters rather than a real parser. This is a
//! deliberate, bounded heuristic — not a one-regex stub — but it has known
//! gaps, documented per rule below. Detection reasoning is adapted from
//! zizmor's `template-injection`, `github-env`, `cache-poisoning`,
//! `dangerous-triggers`, `bot-conditions`, `unsound-condition`,
//! `unsound-contains` and `unsound-ternary` audits (MIT reimplementation of
//! Apache-2.0-licensed detection logic; no zizmor code or data files are
//! embedded or linked).

use serde_json::Value;

use crate::{AuditContext, Confidence, Diagnostic, Persona, Severity};

pub(crate) fn check(doc: &Value, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    check_dangerous_triggers(doc, out);
    check_bot_conditions(doc, out);
    check_template_injection(doc, ctx, out);
    check_github_env(doc, out);
    check_cache_poisoning(doc, out);
    check_unsound_condition(doc, out);
    check_unsound_contains(doc, out);
    check_unsound_ternary(doc, out);
}

// ---------------------------------------------------------------------
// Shared document helpers
// ---------------------------------------------------------------------

/// Only a literal `false` disables a job/step for these audits' purposes,
/// mirroring `preloop-gha-lint`'s documented `if:` semantics: no expression
/// evaluation occurs even for otherwise-constant-looking conditions, and a
/// quoted `${{ false }}` is NOT a literal false (it is exactly the
/// `unsound-condition` footgun this module also detects).
fn is_disabled(if_value: Option<&Value>) -> bool {
    match if_value {
        Some(Value::Bool(false)) => true,
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            !trimmed.starts_with("${{") && trimmed.eq_ignore_ascii_case("false")
        }
        _ => false,
    }
}

fn trigger_present(on: &Value, name: &str) -> bool {
    match on {
        Value::String(single) => single == name,
        Value::Array(items) => items.iter().any(|item| item.as_str() == Some(name)),
        Value::Object(map) => map.contains_key(name),
        _ => false,
    }
}

fn push_has_tag_filter(on: &Value) -> bool {
    let Value::Object(map) = on else { return false };
    let Some(Value::Object(push)) = map.get("push") else {
        return false;
    };
    push.contains_key("tags") || push.contains_key("tags-ignore")
}

fn uses_matches(uses: &str, target: &str) -> bool {
    let name = uses.split('@').next().unwrap_or(uses);
    name.eq_ignore_ascii_case(target)
}

fn action_name(uses: &str) -> String {
    uses.split('@').next().unwrap_or(uses).to_ascii_lowercase()
}

// ---------------------------------------------------------------------
// Expression-text helpers (no AST available; quote/paren aware)
// ---------------------------------------------------------------------

/// Extract every `${{ ... }}` span in `text`, quote-aware so a `}}` inside a
/// single-quoted string literal does not terminate the fence early. Returns
/// `(byte_offset_of_dollar, trimmed_inner_text)` pairs. An unterminated
/// trailing fence is dropped (nothing further to analyze).
fn extract_fenced(text: &str) -> Vec<(usize, &str)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 3 <= bytes.len() {
        if &bytes[i..i + 3] == b"${{" {
            let start = i;
            let mut j = i + 3;
            let mut in_str = false;
            let mut close = None;
            while j < bytes.len() {
                let c = bytes[j];
                if in_str {
                    if c == b'\'' {
                        if j + 1 < bytes.len() && bytes[j + 1] == b'\'' {
                            j += 2;
                            continue;
                        }
                        in_str = false;
                    }
                    j += 1;
                    continue;
                }
                if c == b'\'' {
                    in_str = true;
                    j += 1;
                    continue;
                }
                if c == b'}' && j + 1 < bytes.len() && bytes[j + 1] == b'}' {
                    close = Some(j + 2);
                    break;
                }
                j += 1;
            }
            match close {
                Some(end) => {
                    out.push((start, text[start + 3..end - 2].trim()));
                    i = end;
                }
                None => break,
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Blank out single-quoted string contents (keeping byte length roughly
/// intact) so identifier/context scanning does not mistake string literal
/// text for a context path.
fn strip_quoted(expr: &str) -> String {
    let mut out = String::with_capacity(expr.len());
    let mut chars = expr.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\'' {
            out.push(c);
            continue;
        }
        out.push(' ');
        while let Some(n) = chars.next() {
            if n == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    out.push(' ');
                    continue;
                }
                out.push(' ');
                break;
            }
            out.push(' ');
        }
    }
    out
}

/// Split `text` on every top-level occurrence of `op`, respecting
/// single-quoted strings and `(`/`[` nesting depth.
fn split_top_level<'a>(text: &'a str, op: &str) -> Vec<&'a str> {
    let bytes = text.as_bytes();
    let op_bytes = op.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if c == b'\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' => {
                in_str = true;
                i += 1;
            }
            b'(' | b'[' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' => {
                depth -= 1;
                i += 1;
            }
            _ => {
                if depth <= 0
                    && i + op_bytes.len() <= bytes.len()
                    && &bytes[i..i + op_bytes.len()] == op_bytes
                {
                    parts.push(text[start..i].trim());
                    i += op_bytes.len();
                    start = i;
                } else {
                    i += 1;
                }
            }
        }
    }
    parts.push(text[start..].trim());
    parts
}

fn strip_parens(text: &str) -> &str {
    let trimmed = text.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('(') && trimmed.ends_with(')') {
        let inner = &trimmed[1..trimmed.len() - 1];
        let mut depth = 0i32;
        for c in inner.chars() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        return trimmed;
                    }
                }
                _ => {}
            }
        }
        if depth == 0 {
            return inner.trim();
        }
    }
    trimmed
}

fn is_quoted_literal(s: &str) -> bool {
    s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'')
}

fn unquote(s: &str) -> Option<String> {
    if !is_quoted_literal(s) {
        return None;
    }
    Some(s[1..s.len() - 1].replace("''", "'"))
}

/// A constant literal that GitHub Actions truthiness treats as falsy: the
/// empty string, the boolean `false`, `null`, or numeric `0`. A *quoted*
/// non-empty string is never falsy, even `'0'` or `'false'` — only
/// `is_truthy`'s string-emptiness rule matters there.
fn is_falsy_literal(s: &str) -> bool {
    let t = s.trim();
    if t.eq_ignore_ascii_case("false") || t.eq_ignore_ascii_case("null") {
        return true;
    }
    if t == "''" || t == "\"\"" {
        return true;
    }
    t.parse::<f64>().is_ok_and(|n| n == 0.0)
}

/// Collect lowercase dotted-identifier chains (`github.event.issue.title`)
/// found outside string literals. Index/computed access (`secrets['X']`,
/// `fromJSON(x).y`) and function names are not resolved to paths; this only
/// recovers literal dotted member chains, which covers the overwhelming
/// majority of real-world attacker-controlled-context usage.
fn referenced_paths(expr: &str) -> Vec<String> {
    let stripped = strip_quoted(expr);
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut dots = 0usize;
    for c in stripped.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
            if c == '.' {
                dots += 1;
            }
            current.push(c);
        } else {
            if dots > 0 {
                let trimmed = current.trim_matches('.');
                if !trimmed.is_empty() {
                    paths.push(trimmed.to_ascii_lowercase());
                }
            }
            current.clear();
            dots = 0;
        }
    }
    if dots > 0 {
        let trimmed = current.trim_matches('.');
        if !trimmed.is_empty() {
            paths.push(trimmed.to_ascii_lowercase());
        }
    }
    paths
}

/// Well-known attacker-controllable expression contexts: fields an external,
/// unauthenticated actor can set verbatim by opening an issue/PR, posting a
/// comment, pushing a branch, etc. Curated from GitHub's own "keeping your
/// GitHub Actions and workflows secure" guidance and zizmor's CodeQL-derived
/// sink/context data — **known gap**: this is a hand-curated subset, not the
/// full CodeQL context-capability database zizmor builds at compile time, so
/// some real sinks (e.g. less common webhook payload fields) are not caught.
const ATTACKER_CONTROLLED_PREFIXES: &[&str] = &[
    "github.event.issue.title",
    "github.event.issue.body",
    "github.event.pull_request.title",
    "github.event.pull_request.body",
    "github.event.pull_request.head.ref",
    "github.event.pull_request.head.label",
    "github.event.pull_request.head.repo.full_name",
    "github.event.comment.body",
    "github.event.review.body",
    "github.event.review_comment.body",
    "github.event.discussion.title",
    "github.event.discussion.body",
    "github.event.pages",
    "github.event.commits",
    "github.event.head_commit.message",
    "github.event.head_commit.author.email",
    "github.event.head_commit.author.name",
    "github.event.workflow_run.head_branch",
    "github.event.workflow_run.display_title",
    "github.event.release.body",
    "github.event.release.name",
    "github.event.release.tag_name",
    "github.event.label.name",
    "github.event.milestone.title",
    "github.event.milestone.description",
    "github.head_ref",
];

fn is_attacker_controlled(path: &str) -> bool {
    ATTACKER_CONTROLLED_PREFIXES
        .iter()
        .any(|prefix| match path.strip_prefix(prefix) {
            Some(rest) => rest.is_empty() || rest.starts_with('.'),
            None => false,
        })
}

/// Broader "user-influenceable in common CI setups" context list used only
/// by `unsound-contains`, matching zizmor's own (intentionally wider, lower
/// bar) list for that audit.
const USER_CONTROLLABLE_CONTEXTS: &[&str] = &[
    "env",
    "github.actor",
    "github.base_ref",
    "github.head_ref",
    "github.ref",
    "github.ref_name",
    "github.sha",
    "github.triggering_actor",
    "inputs",
];

fn is_user_controllable(path: &str) -> bool {
    USER_CONTROLLABLE_CONTEXTS
        .iter()
        .any(|prefix| match path.strip_prefix(prefix) {
            Some(rest) => rest.is_empty() || rest.starts_with('.'),
            None => false,
        })
}

/// If `raw` is a job/step `if:` string, return the expression body to
/// analyze: the fenced inner text if explicitly fenced, otherwise the bare
/// string itself (GitHub implicitly treats an un-fenced `if:` value as an
/// expression).
fn fenced_or_bare(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("${{") {
        extract_fenced(trimmed)
            .into_iter()
            .next()
            .map(|(_, inner)| inner.to_owned())
    } else {
        Some(trimmed.to_owned())
    }
}

fn walk_strings(value: &Value, path: &str, f: &mut dyn FnMut(&str, &str)) {
    match value {
        Value::String(s) => f(s, path),
        Value::Object(map) => {
            for (k, v) in map {
                walk_strings(v, &crate::pointer(path, k), f);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                walk_strings(v, &crate::pointer(path, &i.to_string()), f);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------
// dangerous-triggers
// ---------------------------------------------------------------------

const DANGEROUS_TRIGGERS: &[(&str, &str)] = &[
    (
        "pull_request_target",
        "pull_request_target runs with write access and fork secrets against untrusted PR content; it is almost always used insecurely",
    ),
    (
        "workflow_run",
        "workflow_run runs with the base repository's privileges and can be chained from an untrusted workflow_run; it is almost always used insecurely",
    ),
    (
        "issue_comment",
        "issue_comment fires for comments on both issues and PRs (including forked PRs) and is almost always used insecurely",
    ),
];

fn check_dangerous_triggers(doc: &Value, out: &mut Vec<Diagnostic>) {
    let Some(on) = doc.get("on") else { return };
    let labeler_exception = is_labeler_exception(doc);

    for (name, msg) in DANGEROUS_TRIGGERS {
        if !trigger_present(on, name) {
            continue;
        }
        if *name == "pull_request_target" && labeler_exception {
            continue;
        }
        let mut d = crate::finding(
            "dangerous-triggers",
            "/on",
            format!("workflow trigger `{name}`: {msg}"),
            "avoid this trigger for untrusted content; if it is required, keep privileged steps (secrets, write-scope tokens, checkout of the PR head) gated behind manual review",
        );
        d.severity = Severity::Error;
        d.confidence = Confidence::Medium;
        out.push(d);
    }
}

/// A single job with a single `actions/labeler` step is a blessed,
/// presumed-safe use of `pull_request_target` (it only reads PR metadata to
/// apply labels, never checks out or executes PR content).
fn is_labeler_exception(doc: &Value) -> bool {
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return false;
    };
    if jobs.len() != 1 {
        return false;
    }
    let Some(job) = jobs.values().next() else {
        return false;
    };
    let Some(steps) = job.get("steps").and_then(Value::as_array) else {
        return false;
    };
    let [step] = steps.as_slice() else {
        return false;
    };
    step.get("uses")
        .and_then(Value::as_str)
        .is_some_and(|uses| uses_matches(uses, "actions/labeler"))
}

// ---------------------------------------------------------------------
// bot-conditions
// ---------------------------------------------------------------------

const SPOOFABLE_ACTOR_NAME_CONTEXTS: &[&str] = &["github.actor", "github.triggering_actor"];
const SPOOFABLE_ACTOR_ID_CONTEXT: &str = "github.actor_id";

/// Known bot actor/integration IDs; there is no `[bot]`-suffix equivalent
/// check for numeric IDs, so these must be hardcoded.
const BOT_ACTOR_IDS: &[&str] = &[
    "29110",    // dependabot[bot]'s integration ID
    "49699333", // dependabot[bot]
    "27856297", // dependabot-preview[bot]
    "29139614", // renovate[bot]
];

fn check_bot_conditions(doc: &Value, out: &mut Vec<Diagnostic>) {
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        let job_path = crate::pointer("/jobs", job_id);

        if !is_disabled(job.get("if"))
            && let Some(cond) = job.get("if").and_then(Value::as_str)
        {
            check_bot_condition_text(cond, &crate::pointer(&job_path, "if"), out);
        }

        let Some(steps) = job.get("steps").and_then(Value::as_array) else {
            continue;
        };
        let steps_path = crate::pointer(&job_path, "steps");
        for (i, step) in steps.iter().enumerate() {
            if is_disabled(step.get("if")) {
                continue;
            }
            if let Some(cond) = step.get("if").and_then(Value::as_str) {
                let step_path = crate::pointer(&steps_path, &i.to_string());
                check_bot_condition_text(cond, &crate::pointer(&step_path, "if"), out);
            }
        }
    }
}

fn check_bot_condition_text(raw: &str, path: &str, out: &mut Vec<Diagnostic>) {
    let Some(body) = fenced_or_bare(raw) else {
        return;
    };
    let Some((context, confidence)) = bot_condition(&body) else {
        return;
    };
    let mut d = crate::finding(
        "bot-conditions",
        path,
        format!(
            "actor context `{context}` may be spoofable: it reflects the last actor to act on the ref, not necessarily the one who triggered the event, so an attacker can craft a PR/branch where this check passes"
        ),
        "use the event-specific actor context instead (e.g. github.event.pull_request.user.login, github.event.comment.user.login), which reflects who actually created the triggering object",
    );
    d.severity = Severity::Error;
    d.confidence = confidence;
    out.push(d);
}

/// Find a `<spoofable-context> == '<bot-literal>'` check anywhere in `expr`,
/// and report whether it "dominates": reachable from the root through `||`
/// only (so the check alone can make the whole condition pass), versus
/// buried inside an `&&` (still worth flagging, but a false actor alone is
/// not sufficient — lower confidence). Mirrors zizmor's documented
/// dominance test cases exactly.
fn bot_condition(expr: &str) -> Option<(String, Confidence)> {
    for branch in split_top_level(expr, "||") {
        let and_parts = split_top_level(branch, "&&");
        for part in &and_parts {
            if let Some(ctx) = bot_equality(part) {
                let confidence = if and_parts.len() == 1 {
                    Confidence::High
                } else {
                    Confidence::Medium
                };
                return Some((ctx, confidence));
            }
        }
    }
    None
}

fn bot_equality(text: &str) -> Option<String> {
    let text = strip_parens(text);
    let eq_parts = split_top_level(text, "==");
    if eq_parts.len() != 2 {
        return None;
    }
    let (a, b) = (eq_parts[0].trim(), eq_parts[1].trim());
    let (ctx, lit) = if is_quoted_literal(a) {
        (b, a)
    } else if is_quoted_literal(b) {
        (a, b)
    } else {
        return None;
    };
    let literal = unquote(lit)?;
    let ctx_lower = ctx.to_ascii_lowercase();

    if SPOOFABLE_ACTOR_NAME_CONTEXTS.contains(&ctx_lower.as_str()) && literal.ends_with("[bot]") {
        return Some(ctx.to_owned());
    }
    if ctx_lower == SPOOFABLE_ACTOR_ID_CONTEXT && BOT_ACTOR_IDS.contains(&literal.as_str()) {
        return Some(ctx.to_owned());
    }
    None
}

// ---------------------------------------------------------------------
// template-injection
// ---------------------------------------------------------------------

/// Small, curated set of `uses:` inputs that are known code/command
/// execution sinks beyond plain `run:` shell scripts. **Known gap**: zizmor
/// derives a much larger sink list from CodeQL's Action models at build
/// time; this is a hand-picked subset of the most common ones plus the
/// action our fixtures exercise (`mikefarah/yq`'s `cmd:`), not a database.
const ACTION_INJECTION_SINKS: &[(&str, &[&str])] = &[
    ("actions/github-script", &["script"]),
    ("mikefarah/yq", &["cmd"]),
    ("amadevus/pwsh-script", &["script"]),
    ("jannekem/run-python-script-action", &["script"]),
    ("cardinalby/js-eval-action", &["expression"]),
    ("addnab/docker-run-action", &["options", "run"]),
];

fn check_template_injection(doc: &Value, ctx: &AuditContext, out: &mut Vec<Diagnostic>) {
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        let job_path = crate::pointer("/jobs", job_id);
        if is_disabled(job.get("if")) {
            continue;
        }
        let Some(steps) = job.get("steps").and_then(Value::as_array) else {
            continue;
        };
        let steps_path = crate::pointer(&job_path, "steps");

        for (i, step) in steps.iter().enumerate() {
            if is_disabled(step.get("if")) {
                continue;
            }
            let step_path = crate::pointer(&steps_path, &i.to_string());
            let step_env = step.get("env").and_then(Value::as_object);

            if let Some(run) = step.get("run").and_then(Value::as_str) {
                check_run_injection(run, &crate::pointer(&step_path, "run"), step_env, ctx, out);
            }

            if let Some(uses) = step.get("uses").and_then(Value::as_str) {
                let action = action_name(uses);
                if let Some((_, inputs)) = ACTION_INJECTION_SINKS
                    .iter()
                    .find(|(name, _)| *name == action)
                    && let Some(with) = step.get("with").and_then(Value::as_object)
                {
                    let with_path = crate::pointer(&step_path, "with");
                    for input_name in inputs.iter().copied() {
                        if let Some(value) = with.get(input_name).and_then(Value::as_str) {
                            check_sink_injection(
                                value,
                                &crate::pointer(&with_path, input_name),
                                &action,
                                input_name,
                                out,
                            );
                        }
                    }
                }
            }
        }
    }
}

fn check_run_injection(
    run: &str,
    path: &str,
    step_env: Option<&serde_json::Map<String, Value>>,
    ctx: &AuditContext,
    out: &mut Vec<Diagnostic>,
) {
    for (_, body) in extract_fenced(run) {
        let paths = referenced_paths(body);

        if let Some(attacker_path) = paths.iter().find(|p| is_attacker_controlled(p)) {
            push_run_injection(path, body, attacker_path, out);
            continue;
        }

        if let Some(env_name) = env_reference(body)
            && let Some(env_expr) = step_env
                .and_then(|m| m.get(&env_name))
                .and_then(Value::as_str)
        {
            let mut flagged = false;
            for (_, env_body) in extract_fenced(env_expr) {
                let env_paths = referenced_paths(env_body);
                if let Some(attacker_path) = env_paths.iter().find(|p| is_attacker_controlled(p)) {
                    let mut d = crate::finding(
                        "template-injection",
                        path,
                        format!(
                            "`${{{{ env.{env_name} }}}}` expands an attacker-controllable value (propagated from `env.{env_name}: ${{{{ {env_body} }}}}`, which reads `{attacker_path}`) directly inside a run step"
                        ),
                        "reference the variable as a shell variable (`$NAME`/`${NAME}`), not `${{ env.NAME }}` — templating env.* is still a template expansion, not a shell-safe substitution",
                    );
                    d.severity = Severity::Error;
                    d.confidence = Confidence::High;
                    out.push(d);
                    flagged = true;
                    break;
                }
            }
            if flagged {
                continue;
            }
        }

        if ctx.persona != Persona::Regular {
            let mut d = crate::finding(
                "template-injection",
                path,
                format!(
                    "template expansion `${{{{ {body} }}}}` inside a run step is a code smell even when the referenced context looks safe today"
                ),
                "prefer passing the value through `env:` and referencing it as a shell variable, never `${{ }}`, inside code-execution contexts",
            );
            d.severity = Severity::Info;
            d.confidence = Confidence::Medium;
            out.push(d);
        }
    }
}

fn push_run_injection(path: &str, body: &str, attacker_path: &str, out: &mut Vec<Diagnostic>) {
    let mut d = crate::finding(
        "template-injection",
        path,
        format!(
            "`${{{{ {body} }}}}` expands an attacker-controllable value (`{attacker_path}`) directly inside a run step, which can inject arbitrary shell syntax"
        ),
        "pass the value through `env:` and reference it as a shell variable (`$NAME`/`${NAME}`) instead of templating it directly into the script",
    );
    d.severity = Severity::Error;
    d.confidence = Confidence::High;
    out.push(d);
}

fn check_sink_injection(
    value: &str,
    path: &str,
    action: &str,
    input: &str,
    out: &mut Vec<Diagnostic>,
) {
    for (_, body) in extract_fenced(value) {
        let paths = referenced_paths(body);
        let Some(attacker_path) = paths.iter().find(|p| is_attacker_controlled(p)) else {
            continue;
        };
        let mut d = crate::finding(
            "template-injection",
            path,
            format!(
                "`${{{{ {body} }}}}` expands an attacker-controllable value (`{attacker_path}`) into `{action}`'s `{input}:` input, a known code-execution sink"
            ),
            "pass the value through an intermediate environment variable instead of templating it directly into the sink input",
        );
        d.severity = Severity::Error;
        d.confidence = Confidence::High;
        out.push(d);
    }
}

/// If `expr` (trimmed, quote-stripped) is *exactly* `env.NAME`, return
/// `NAME` with its original case (env var names are case-sensitive on
/// POSIX runners). Only the single-reference form is recognized — **known
/// gap**: `${{ format('{0}', env.NAME) }}` or similar indirections are not
/// traced.
fn env_reference(expr: &str) -> Option<String> {
    let stripped = strip_quoted(expr);
    let trimmed = stripped.trim();
    if trimmed.len() <= 4 {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("env.") {
        return None;
    }
    let name = &trimmed[4..];
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(name.to_owned())
}

// ---------------------------------------------------------------------
// github-env
// ---------------------------------------------------------------------

const GITHUB_ENV_DESTINATIONS: &[&str] = &["GITHUB_ENV", "GITHUB_PATH"];

fn check_github_env(doc: &Value, out: &mut Vec<Diagnostic>) {
    let Some(on) = doc.get("on") else { return };
    let dangerous =
        trigger_present(on, "pull_request_target") || trigger_present(on, "workflow_run");
    if !dangerous {
        return;
    }

    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        let job_path = crate::pointer("/jobs", job_id);
        if is_disabled(job.get("if")) {
            continue;
        }
        let Some(steps) = job.get("steps").and_then(Value::as_array) else {
            continue;
        };
        let steps_path = crate::pointer(&job_path, "steps");

        for (i, step) in steps.iter().enumerate() {
            if is_disabled(step.get("if")) {
                continue;
            }
            let Some(run) = step.get("run").and_then(Value::as_str) else {
                continue;
            };
            let step_path = crate::pointer(&steps_path, &i.to_string());
            for dest in dynamic_github_env_writes(run) {
                let mut d = crate::finding(
                    "github-env",
                    &crate::pointer(&step_path, "run"),
                    format!(
                        "write to {dest} includes dynamic content (shell expansion, command substitution, or another variable) and may allow code execution in subsequent steps"
                    ),
                    "avoid writing computed or attacker-influenced content to GITHUB_ENV/GITHUB_PATH in a workflow reachable by pull_request_target/workflow_run; stick to literal strings, or use GITHUB_OUTPUT to pass state between steps",
                );
                d.severity = Severity::Error;
                d.confidence = Confidence::Medium;
                out.push(d);
            }
        }
    }
}

/// Scan `script` line-by-line (shell-agnostic: covers bash/sh/zsh `$VAR`,
/// pwsh `$env:VAR`, and cmd `%VAR%` spellings of the destination) for a
/// redirect into `GITHUB_ENV`/`GITHUB_PATH` whose written content is not
/// provably a literal. **Known gap**: this is per-line, so a heredoc body
/// spanning multiple lines (`cat >> $GITHUB_ENV << EOF`) is not traced past
/// its opening line, and Python `run:` steps are not specially handled.
fn dynamic_github_env_writes(script: &str) -> Vec<&'static str> {
    let mut hits: Vec<&'static str> = Vec::new();
    for line in script.lines() {
        if !line.contains('>') {
            continue;
        }
        for dest in GITHUB_ENV_DESTINATIONS {
            if hits.contains(dest) {
                continue;
            }
            let markers = [
                format!("${dest}"),
                format!("${{{dest}}}"),
                format!("%{dest}%"),
                format!("$env:{dest}"),
            ];
            let Some(marker) = markers.iter().find(|m| line.contains(m.as_str())) else {
                continue;
            };
            let scrubbed = line.replacen(marker.as_str(), "", 1);
            let percent_pairs = scrubbed.matches('%').count();
            let is_dynamic = scrubbed.contains('$') || scrubbed.contains('`') || percent_pairs >= 2;
            if is_dynamic {
                hits.push(dest);
            }
        }
    }
    hits
}

// ---------------------------------------------------------------------
// cache-poisoning
// ---------------------------------------------------------------------

/// Whether `action` (lowercase, no ref) restores from its cache in this
/// step given its `with:` inputs, and if so whether caching is effectively
/// on. `None` means the action is not a recognized cache-aware action.
/// **Known gap**: curated list of common cache-aware actions (the ones
/// zizmor's own docs call out by name), not an exhaustive registry — a
/// third-party action with undocumented built-in caching is invisible here.
fn cache_enabled(action: &str, with: Option<&serde_json::Map<String, Value>>) -> Option<bool> {
    let input = |key: &str| with.and_then(|m| m.get(key));
    match action {
        "actions/cache" => Some(!matches!(input("lookup-only"), Some(Value::Bool(true)))),
        "actions/setup-java"
        | "actions/setup-node"
        | "actions/setup-python"
        | "actions/setup-dotnet" => Some(match input("cache") {
            None => false,
            Some(Value::Bool(false)) => false,
            Some(Value::String(s)) if s.is_empty() => false,
            Some(_) => true,
        }),
        "actions/setup-go" => Some(!matches!(input("cache"), Some(Value::Bool(false)))),
        "ruby/setup-ruby" => Some(matches!(input("bundler-cache"), Some(Value::Bool(true)))),
        "astral-sh/setup-uv" => Some(!matches!(input("enable-cache"), Some(Value::Bool(false)))),
        "swatinem/rust-cache" => {
            if matches!(input("lookup-only"), Some(Value::Bool(true))) {
                return Some(false);
            }
            if matches!(input("cache"), Some(Value::Bool(false))) {
                return Some(false);
            }
            Some(true)
        }
        _ => None,
    }
}

fn check_cache_poisoning(doc: &Value, out: &mut Vec<Diagnostic>) {
    let Some(on) = doc.get("on") else { return };
    let release_sensitive = trigger_present(on, "release") || push_has_tag_filter(on);
    let dangerous_trigger =
        trigger_present(on, "pull_request_target") || trigger_present(on, "workflow_run");
    if !release_sensitive && !dangerous_trigger {
        return;
    }

    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        let job_path = crate::pointer("/jobs", job_id);
        if is_disabled(job.get("if")) {
            continue;
        }
        let Some(steps) = job.get("steps").and_then(Value::as_array) else {
            continue;
        };
        let steps_path = crate::pointer(&job_path, "steps");

        for (i, step) in steps.iter().enumerate() {
            if is_disabled(step.get("if")) {
                continue;
            }
            let Some(uses) = step.get("uses").and_then(Value::as_str) else {
                continue;
            };
            let action = action_name(uses);
            let with = step.get("with").and_then(Value::as_object);
            let Some(true) = cache_enabled(&action, with) else {
                continue;
            };

            let step_path = crate::pointer(&steps_path, &i.to_string());
            let uses_path = crate::pointer(&step_path, "uses");

            if release_sensitive {
                let mut d = crate::finding(
                    "cache-poisoning",
                    &uses_path,
                    format!(
                        "`{action}` restores from the GitHub Actions cache in a release-sensitive workflow; a cache entry poisoned by an earlier, lower-trust build could be restored into this release"
                    ),
                    "disable caching in release workflows (the action's own disable input, e.g. lookup-only/enable-cache: false), or remove the cache-aware action from the release job",
                );
                d.severity = Severity::Warning;
                d.confidence = Confidence::Medium;
                out.push(d);
            }
            if dangerous_trigger {
                let mut d = crate::finding(
                    "cache-poisoning",
                    &uses_path,
                    format!(
                        "`{action}` can write to the GitHub Actions cache from a job reachable by an untrusted trigger, letting an attacker poison the cache for a later, higher-privilege workflow that restores it"
                    ),
                    "disable caching for this trigger, or gate the cache-aware step behind an `if:` that excludes the untrusted path",
                );
                d.severity = Severity::Warning;
                d.confidence = Confidence::Medium;
                out.push(d);
            }
        }
    }
}

// ---------------------------------------------------------------------
// unsound-condition
// ---------------------------------------------------------------------

fn check_unsound_condition(doc: &Value, out: &mut Vec<Diagnostic>) {
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        let job_path = crate::pointer("/jobs", job_id);
        if let Some(Value::String(s)) = job.get("if") {
            check_unsound_condition_text(s, &crate::pointer(&job_path, "if"), out);
        }
        let Some(steps) = job.get("steps").and_then(Value::as_array) else {
            continue;
        };
        let steps_path = crate::pointer(&job_path, "steps");
        for (i, step) in steps.iter().enumerate() {
            if let Some(Value::String(s)) = step.get("if") {
                let step_path = crate::pointer(&steps_path, &i.to_string());
                check_unsound_condition_text(s, &crate::pointer(&step_path, "if"), out);
            }
        }
    }
}

/// Detects the classic multiline-block-scalar-plus-fenced-expression
/// footgun: `if: |\n  ${{ false }}\n` parses to the string `"${{ false
/// }}\n"`, whose trailing newline sits *outside* the expression fence from
/// the expression engine's point of view, so GitHub expands it to the
/// string `"false\n"` — non-empty, hence truthy. **Known gap**: only the
/// common trailing-content case is checked (no leading whitespace before
/// the fence — block scalars strip indentation, so this is the realistic
/// shape); a fenced expression preceded by other literal text is not
/// separately diagnosed here.
fn check_unsound_condition_text(raw: &str, path: &str, out: &mut Vec<Diagnostic>) {
    if !raw.starts_with("${{") {
        return;
    }
    let Some(close_rel) = raw.find("}}") else {
        return;
    };
    let fence_len = close_rel + 2;
    if raw.len() == fence_len {
        return;
    }

    let mut d = crate::finding(
        "unsound-condition",
        path,
        "condition is a fenced expression with extra trailing content (likely a multiline block scalar's trailing newline), so it always evaluates to a truthy string regardless of the expression's value",
        "write a bare expression without `${{ }}`, or use a block scalar style that strips the trailing newline (`|-`/`>-`)",
    );
    d.severity = Severity::Error;
    d.confidence = Confidence::High;
    out.push(d);
}

// ---------------------------------------------------------------------
// unsound-contains
// ---------------------------------------------------------------------

fn check_unsound_contains(doc: &Value, out: &mut Vec<Diagnostic>) {
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else {
        return;
    };
    for (job_id, job) in jobs {
        let job_path = crate::pointer("/jobs", job_id);
        if !is_disabled(job.get("if"))
            && let Some(cond) = job.get("if").and_then(Value::as_str)
        {
            check_unsound_contains_text(cond, &crate::pointer(&job_path, "if"), out);
        }
        let Some(steps) = job.get("steps").and_then(Value::as_array) else {
            continue;
        };
        let steps_path = crate::pointer(&job_path, "steps");
        for (i, step) in steps.iter().enumerate() {
            if is_disabled(step.get("if")) {
                continue;
            }
            if let Some(cond) = step.get("if").and_then(Value::as_str) {
                let step_path = crate::pointer(&steps_path, &i.to_string());
                check_unsound_contains_text(cond, &crate::pointer(&step_path, "if"), out);
            }
        }
    }
}

fn check_unsound_contains_text(raw: &str, path: &str, out: &mut Vec<Diagnostic>) {
    let Some(body) = fenced_or_bare(raw) else {
        return;
    };
    for (literal, ctx) in find_contains_calls(&body) {
        let ctx_lower = ctx.to_ascii_lowercase();
        let user_controllable = is_user_controllable(&ctx_lower);
        let mut d = crate::finding(
            "unsound-contains",
            path,
            format!(
                "contains('{literal}', {ctx}) is a substring check, not an equality check: it can be bypassed if an attacker controls the value of `{ctx}`"
            ),
            "use an exact comparison (==) or an explicit list-membership check instead of contains() when the needle is attacker-influenceable",
        );
        d.severity = if user_controllable {
            Severity::Error
        } else {
            Severity::Info
        };
        d.confidence = Confidence::High;
        out.push(d);
    }
}

/// Find `contains('<literal>', <context>)` calls — literal search string
/// first, context needle second, matching GitHub's actual `contains(search,
/// item)` argument order. The reversed form (`contains(ctx, 'literal')`) is
/// an ordinary, sound substring check and is intentionally not flagged.
/// **Known gap**: a `contains()` call nested inside another `contains()`'s
/// arguments is skipped once the outer call is matched (no AST to resume
/// descent from).
fn find_contains_calls(expr: &str) -> Vec<(String, String)> {
    let mut results = Vec::new();
    let lower = expr.to_ascii_lowercase();
    let mut search_from = 0usize;
    while let Some(rel) = lower[search_from..].find("contains(") {
        let start = search_from + rel;
        let before_ok = start == 0
            || !(expr.as_bytes()[start - 1] as char).is_ascii_alphanumeric()
                && expr.as_bytes()[start - 1] != b'_';
        let args_start = start + "contains(".len();
        let Some(args_end) = matching_paren(expr, args_start) else {
            break;
        };
        if before_ok {
            let args_text = &expr[args_start..args_end];
            let parts = split_top_level(args_text, ",");
            if parts.len() == 2 {
                let lit = parts[0].trim();
                let ctx = parts[1].trim();
                if is_quoted_literal(lit)
                    && !ctx.is_empty()
                    && !is_quoted_literal(ctx)
                    && let Some(literal) = unquote(lit)
                {
                    results.push((literal, ctx.to_owned()));
                }
            }
        }
        search_from = args_end + 1;
    }
    results
}

/// `open_idx` points just past an already-consumed `(`; returns the index
/// of its matching `)`, quote-aware.
fn matching_paren(text: &str, open_idx: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 1i32;
    let mut i = open_idx;
    let mut in_str = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if c == b'\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' => {
                in_str = true;
                i += 1;
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

// ---------------------------------------------------------------------
// unsound-ternary
// ---------------------------------------------------------------------

/// Detects `cond && <falsy-literal> || <fallback>` pseudo-ternaries: the
/// author intends "truthy-value if cond else fallback", but if the
/// truthy-value literal is itself falsy (`''`, `false`, `0`, `null`), the
/// `&&` short-circuits to the falsy value and the whole operand is false
/// even when `cond` holds, so the expression *always* evaluates to
/// `fallback`. Scanned across every string field in the document (not just
/// `if:`), since the idiom shows up in `run:`, `with:` and `env:` just as
/// often. **Known gap**: only matches the literal `&&`/`||` shape at the
/// textual top level of each fenced expression — a ternary built from
/// nested function calls or nested nested `||` won't recurse past the first
/// `&&`-split per branch (matching zizmor's own non-exhaustive AST walk).
fn check_unsound_ternary(doc: &Value, out: &mut Vec<Diagnostic>) {
    let mut findings = Vec::new();
    walk_strings(doc, "", &mut |text, path| {
        for (_, body) in extract_fenced(text) {
            check_unsound_ternary_expr(body, path, &mut findings);
        }
    });
    out.extend(findings);
}

fn check_unsound_ternary_expr(expr: &str, path: &str, out: &mut Vec<Diagnostic>) {
    let branches = split_top_level(expr, "||");
    if branches.len() < 2 {
        return;
    }
    for branch in &branches[..branches.len() - 1] {
        let and_parts = split_top_level(branch, "&&");
        if and_parts.len() < 2 {
            continue;
        }
        let true_value = and_parts[and_parts.len() - 1].trim();
        if !is_falsy_literal(true_value) {
            continue;
        }
        let mut d = crate::finding(
            "unsound-ternary",
            path,
            format!(
                "pseudo-ternary has a falsy \"true\" value (`{true_value}`), so the `||` fallback runs even when the guarding condition holds"
            ),
            "use a non-empty, non-zero, non-false literal for the true branch, or write the condition as a real `if:`/nested expression instead of relying on `&&`/`||` short-circuiting",
        );
        d.severity = Severity::Info;
        d.confidence = Confidence::High;
        out.push(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_fenced_handles_quoted_braces() {
        let found = extract_fenced("echo ${{ foo == '}}' }}");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, "foo == '}}'");
    }

    #[test]
    fn split_top_level_respects_quotes_and_parens() {
        let parts = split_top_level("a == 'x || y' || (b && c)", "||");
        assert_eq!(parts, vec!["a == 'x || y'", "(b && c)"]);
    }

    #[test]
    fn referenced_paths_finds_dotted_context() {
        let paths = referenced_paths("github.event.issue.title == 'x'");
        assert!(paths.contains(&"github.event.issue.title".to_owned()));
    }

    #[test]
    fn is_falsy_literal_matches_gha_truthiness() {
        assert!(is_falsy_literal("''"));
        assert!(is_falsy_literal("false"));
        assert!(is_falsy_literal("0"));
        assert!(is_falsy_literal("null"));
        assert!(!is_falsy_literal("'0'"));
        assert!(!is_falsy_literal("'false'"));
        assert!(!is_falsy_literal("'bar'"));
    }
}

//! Dynamically navigable GitHub Actions contexts, ported from the
//! TypeScript SDK's `src/expr/contexts.ts`. Alias tables are copied
//! verbatim.

use std::fmt;

use serde_json::Value;

use super::literal::{operand, Operand};
use super::{precedence, Expr};
use crate::error::Result;

/// Matches property names GitHub accepts in dereference syntax.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Escapes `value` as a GitHub Actions single-quoted string literal.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if c == '\'' {
            out.push_str("''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// A static table mapping camelCase aliases to their exact GitHub keys.
type AliasTable = &'static [(&'static str, &'static str)];

/// A context expression that can be navigated dynamically: [`Context::field`]
/// yields another `Context`, rendered as `base.name` or `base['name']`;
/// [`Context::at`] yields `base[key]` for a fully dynamic key.
///
/// Ported from the TypeScript SDK's `DynExpr`, which uses a `Proxy` to turn
/// arbitrary property reads into member expressions; Rust has no runtime
/// property interception, so `.field(name)`/`.at(key)` are explicit calls
/// instead, and the real `Expr` members (`source()`, `contexts()`,
/// `precedence()`, `bare()`) are reached directly since `Context` composes
/// rather than wraps-and-proxies them.
#[derive(Debug, Clone)]
pub struct Context {
    expr: Expr,
    aliases: AliasTable,
}

impl Context {
    /// Builds a root context expression named `name`. Its `contexts()` set
    /// contains the lowercase form of `name`.
    ///
    /// # Errors
    /// Returns an error when `name` is empty/whitespace or contains the
    /// `${{`/`}}` wrapper delimiters; see [`Expr::raw`].
    pub fn new(name: impl Into<String>) -> Result<Context> {
        Ok(Context {
            expr: Expr::root_context(name.into())?,
            aliases: &[],
        })
    }

    fn with_aliases(name: &'static str, aliases: AliasTable) -> Context {
        let mut context = Context::new(name).expect("built-in context name is always valid");
        context.aliases = aliases;
        context
    }

    /// Builds `base.name` or `base['name']` at Member precedence,
    /// resolving `name` through this context's alias table first (e.g.
    /// `github.field("refName")` resolves to `github.ref_name`). The
    /// returned context carries no alias table of its own, matching the
    /// TypeScript SDK (`github.event` is a plain, alias-free `DynExpr`).
    pub fn field(&self, name: &str) -> Context {
        let resolved = self.resolve_alias(name);
        let source = if is_identifier(resolved) {
            format!("{}.{resolved}", self.expr.source())
        } else {
            format!("{}[{}]", self.expr.source(), quote(resolved))
        };
        Context {
            expr: Expr::composed(source, self.expr.contexts().clone(), precedence::MEMBER),
            aliases: &[],
        }
    }

    /// Builds `base[operand(key)]` at Member precedence, for a key that
    /// isn't a fixed field name (e.g. a loop variable).
    ///
    /// # Errors
    /// Returns [`crate::Error::NonFiniteNumber`] when `key` resolves to a
    /// non-finite float literal.
    pub fn at(&self, key: impl Into<Operand>) -> Result<Context> {
        let key_expr = operand(key)?;
        let mut contexts = self.expr.contexts().clone();
        for name in key_expr.contexts() {
            contexts.insert(name.to_lowercase());
        }
        let source = format!("{}[{}]", self.expr.source(), key_expr.source());
        Ok(Context {
            expr: Expr::composed(source, contexts, precedence::MEMBER),
            aliases: &[],
        })
    }

    fn resolve_alias<'a>(&self, name: &'a str) -> &'a str {
        for (camel, exact) in self.aliases {
            if *camel == name {
                return exact;
            }
        }
        name
    }

    /// The underlying expression.
    pub fn expr(&self) -> &Expr {
        &self.expr
    }

    /// Consumes the context, returning its underlying expression.
    pub fn into_expr(self) -> Expr {
        self.expr
    }

    /// Returns the bare expression text without the `${{ ... }}` wrapper.
    pub fn bare(&self) -> String {
        self.expr.bare()
    }
}

impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.expr.fmt(f)
    }
}

impl From<Context> for Expr {
    fn from(context: Context) -> Expr {
        context.expr
    }
}

impl From<Context> for Operand {
    fn from(context: Context) -> Operand {
        Operand::Expr(context.expr)
    }
}

impl From<Context> for Value {
    fn from(context: Context) -> Value {
        Value::from(context.expr)
    }
}

impl From<&Context> for Value {
    fn from(context: &Context) -> Value {
        Value::from(&context.expr)
    }
}

/// camelCase aliases for the snake_case members of the `github` context.
const GITHUB_ALIASES: AliasTable = &[
    ("actionPath", "action_path"),
    ("actionRef", "action_ref"),
    ("actionRepository", "action_repository"),
    ("actionStatus", "action_status"),
    ("actorId", "actor_id"),
    ("apiUrl", "api_url"),
    ("artifactsList", "artifacts_list"),
    ("baseRef", "base_ref"),
    ("eventName", "event_name"),
    ("eventPath", "event_path"),
    ("graphqlUrl", "graphql_url"),
    ("headRef", "head_ref"),
    ("refName", "ref_name"),
    ("refProtected", "ref_protected"),
    ("refType", "ref_type"),
    ("repositoryId", "repository_id"),
    ("repositoryOwner", "repository_owner"),
    ("repositoryOwnerId", "repository_owner_id"),
    ("repositoryUrl", "repositoryUrl"),
    ("retentionDays", "retention_days"),
    ("runAttempt", "run_attempt"),
    ("runId", "run_id"),
    ("runNumber", "run_number"),
    ("secretSource", "secret_source"),
    ("serverUrl", "server_url"),
    ("triggeringActor", "triggering_actor"),
    ("workflowRef", "workflow_ref"),
    ("workflowSha", "workflow_sha"),
];

/// camelCase aliases for the snake_case members of the `runner` context.
const RUNNER_ALIASES: AliasTable = &[("toolCache", "tool_cache")];

/// camelCase aliases for the kebab-case members of the `strategy` context.
const STRATEGY_ALIASES: AliasTable = &[
    ("failFast", "fail-fast"),
    ("jobIndex", "job-index"),
    ("jobTotal", "job-total"),
    ("maxParallel", "max-parallel"),
];

/// camelCase aliases for the snake_case members of the `job` context.
const JOB_ALIASES: AliasTable = &[
    ("checkRunId", "check_run_id"),
    ("workflowFilePath", "workflow_file_path"),
    ("workflowRef", "workflow_ref"),
    ("workflowRepository", "workflow_repository"),
    ("workflowSha", "workflow_sha"),
];

/// The `github` context: information about the workflow run. Snake_case
/// members are reachable through `.field("ref_name")` and through their
/// camelCase aliases (`.field("refName")`); `event` stays a plain dynamic
/// object.
pub fn github() -> Context {
    Context::with_aliases("github", GITHUB_ALIASES)
}

/// The `env` context: variables set in a workflow, job, or step.
pub fn env() -> Context {
    Context::new("env").expect("\"env\" is a valid context name")
}

/// The `vars` context: configuration variables set at the repository,
/// organization, or environment level.
pub fn vars() -> Context {
    Context::new("vars").expect("\"vars\" is a valid context name")
}

/// The `secrets` context: names and values of secrets available to the
/// run.
pub fn secrets() -> Context {
    Context::new("secrets").expect("\"secrets\" is a valid context name")
}

/// The `inputs` context: inputs of a reusable or manually triggered
/// workflow.
pub fn inputs() -> Context {
    Context::new("inputs").expect("\"inputs\" is a valid context name")
}

/// The `runner` context: information about the runner executing the job.
pub fn runner() -> Context {
    Context::with_aliases("runner", RUNNER_ALIASES)
}

/// The `strategy` context: information about the current matrix strategy.
pub fn strategy() -> Context {
    Context::with_aliases("strategy", STRATEGY_ALIASES)
}

/// The `matrix` context: the user-defined axes of the current matrix
/// combination (`${{ matrix.os }}`). No aliases — every field is
/// user-defined in `strategy.matrix`.
pub fn matrix() -> Context {
    Context::new("matrix").expect("\"matrix\" is a valid context name")
}

/// The `job` context, named `job_context` to leave `job` free for the job
/// builder constructor in [`crate::core`].
pub fn job_context() -> Context {
    Context::with_aliases("job", JOB_ALIASES)
}

/// The `needs` context, used internally to build
/// `needs.<id>.outputs.<name>` / `.result` job references.
pub(crate) fn needs_context() -> Context {
    Context::new("needs").expect("\"needs\" is a valid context name")
}

/// The `steps` context, used internally to build
/// `steps.<id>.outputs.<name>` / `.outcome` / `.conclusion` step
/// references.
pub(crate) fn steps_context() -> Context {
    Context::new("steps").expect("\"steps\" is a valid context name")
}

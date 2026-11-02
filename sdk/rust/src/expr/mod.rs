//! Typed GitHub Actions expression builders: literals, operators, built-in
//! functions, and dynamically navigable contexts. Ported from the
//! TypeScript SDK's `src/kit/expr.ts` and `src/expr/`.

mod contexts;
mod functions;
mod literal;
mod operators;

use std::collections::BTreeSet;
use std::fmt;

use serde_json::Value;

use crate::error::{Error, Result};

pub use contexts::{env, github, inputs, job_context, runner, secrets, strategy, vars, Context};
pub(crate) use contexts::{needs_context, steps_context};
pub use functions::{
    always, cancelled, case_of, contains, ends_with, failure, format, from_json, hash_files, join,
    starts_with, success, to_json,
};
pub use literal::{lit, operand, Literal, Operand};
pub use operators::{and, at, eq, ge, gt, le, lt, ne, not, or, star};

/// Operator precedence levels for GitHub Actions expressions, ordered from
/// the loosest binding (`OR`) to the tightest (`ATOM`).
pub mod precedence {
    /// `a || b`.
    pub const OR: f64 = 1.0;
    /// `a && b`.
    pub const AND: f64 = 2.0;
    /// `a == b`, `a != b`.
    pub const EQUALITY: f64 = 3.0;
    /// `a < b`, `a <= b`, `a > b`, `a >= b`.
    pub const COMPARISON: f64 = 4.0;
    /// `!a`.
    pub const NOT: f64 = 5.0;
    /// `a.b`, `a[b]`, `a.*`.
    pub const MEMBER: f64 = 6.0;
    /// Literals, function calls, parenthesized expressions.
    pub const ATOM: f64 = 7.0;
}

/// A GitHub Actions expression: the bare expression text plus its
/// `${{ ... }}` wrapper, the top-level contexts it references, and its
/// operator precedence.
///
/// `Display`/`to_string()` render the wrapped form (`${{ source }}`) for use
/// as a YAML scalar; [`Expr::bare`] returns the unwrapped source text.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    source: String,
    contexts: BTreeSet<String>,
    precedence: f64,
}

/// Returns the first `${{` or `}}` found outside a single-quoted string
/// literal, or `None` when `source` is clean. GitHub expression strings
/// escape a single quote by doubling it.
fn find_forbidden_delimiter(source: &str) -> Option<&'static str> {
    let chars: Vec<char> = source.chars().collect();
    let mut in_string = false;
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if in_string {
            if ch == '\'' {
                if chars.get(index + 1) == Some(&'\'') {
                    index += 1;
                } else {
                    in_string = false;
                }
            }
            index += 1;
            continue;
        }
        if ch == '\'' {
            in_string = true;
            index += 1;
            continue;
        }
        if matches_at(&chars, index, &['$', '{', '{']) {
            return Some("${{");
        }
        if matches_at(&chars, index, &['}', '}']) {
            return Some("}}");
        }
        index += 1;
    }
    None
}

fn matches_at(chars: &[char], index: usize, pattern: &[char]) -> bool {
    chars.get(index..index + pattern.len()) == Some(pattern)
}

fn is_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Returns the closest non-whitespace character before `index`, or `None`.
fn previous_significant(chars: &[char], index: usize) -> Option<char> {
    if index == 0 {
        return None;
    }
    let mut cursor = index - 1;
    loop {
        if !is_whitespace(chars[cursor]) {
            return Some(chars[cursor]);
        }
        if cursor == 0 {
            return None;
        }
        cursor -= 1;
    }
}

/// Collects the lowercase top-level context names referenced by `source`.
///
/// A top-level context is an identifier followed (after optional
/// whitespace) by `.` or `[`, that is itself not the tail of a member
/// access. Identifiers inside single-quoted string literals are ignored, as
/// are function names (which are followed by an opening parenthesis).
fn detect_contexts(source: &str) -> BTreeSet<String> {
    let chars: Vec<char> = source.chars().collect();
    let mut contexts = BTreeSet::new();
    let mut in_string = false;
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if in_string {
            if ch == '\'' {
                if chars.get(index + 1) == Some(&'\'') {
                    index += 2;
                } else {
                    in_string = false;
                    index += 1;
                }
            } else {
                index += 1;
            }
            continue;
        }
        if ch == '\'' {
            in_string = true;
            index += 1;
            continue;
        }
        if !(ch.is_ascii_alphabetic() || ch == '_') {
            index += 1;
            continue;
        }
        let start = index;
        let mut end = index + 1;
        while end < chars.len() && is_name_char(chars[end]) {
            end += 1;
        }
        let mut lookahead = end;
        while lookahead < chars.len() && is_whitespace(chars[lookahead]) {
            lookahead += 1;
        }
        let next = chars.get(lookahead).copied();
        let previous = previous_significant(&chars, start);
        if (next == Some('.') || next == Some('['))
            && previous != Some('.')
            && previous != Some('[')
            && previous != Some('\'')
        {
            let name: String = chars[start..end].iter().collect();
            contexts.insert(name.to_lowercase());
        }
        index = end;
    }
    contexts
}

impl Expr {
    fn new(source: String, extra_contexts: &[String], precedence: f64) -> Result<Expr> {
        if source.trim().is_empty() {
            return Err(Error::EmptyExpression);
        }
        if let Some(found) = find_forbidden_delimiter(&source) {
            return Err(Error::ForbiddenDelimiter(found));
        }
        if !precedence.is_finite() {
            return Err(Error::NonFinitePrecedence);
        }
        let mut contexts = detect_contexts(&source);
        for name in extra_contexts {
            contexts.insert(name.to_lowercase());
        }
        Ok(Expr {
            source,
            contexts,
            precedence,
        })
    }

    /// Wraps `source` in an `Expr` at the default (Atom) precedence,
    /// recording any top-level contexts it references.
    ///
    /// # Errors
    /// Returns [`Error::EmptyExpression`] when `source` is empty or
    /// whitespace, or [`Error::ForbiddenDelimiter`] when it contains `${{`
    /// or `}}` outside a single-quoted string literal.
    pub fn raw(source: impl Into<String>) -> Result<Expr> {
        Expr::new(source.into(), &[], precedence::ATOM)
    }

    /// Wraps `source` in an `Expr` at an explicit operator precedence, for
    /// composing hand-built fragments with [`and`]/[`or`]/[`not`] and the
    /// comparison operators without an extra, redundant parenthesization.
    ///
    /// # Errors
    /// As [`Expr::raw`], plus [`Error::NonFinitePrecedence`] when
    /// `precedence` is `NaN` or infinite.
    pub fn with_precedence(source: impl Into<String>, precedence: f64) -> Result<Expr> {
        Expr::new(source.into(), &[], precedence)
    }

    /// Builds a root context expression named `name` (e.g. `"github"`),
    /// whose `contexts()` set contains `name`'s lowercase form even though
    /// a bare context name has no following `.`/`[` for [`detect_contexts`]
    /// to pick up on its own.
    pub(crate) fn root_context(name: String) -> Result<Expr> {
        let extra = [name.clone()];
        Expr::new(name, &extra, precedence::ATOM)
    }

    /// Builds an `Expr` from an already-validated `source`/`contexts`/
    /// `precedence` triple, skipping re-validation. Used internally by
    /// operator, function, and context composition, where `source` is
    /// assembled from previously validated sub-expressions and fixed glue
    /// text (`.`, `[`, `]`, `(`, `)`, `, `, operator symbols, `'`-quoted
    /// literals) that can never spell `${{`/`}}` outside a quoted literal
    /// or produce an empty result -- so composing never actually fails, and
    /// a crate-private `Result`-free constructor keeps composition call
    /// sites free of a `?` that could never return `Err`.
    pub(crate) fn composed(source: String, contexts: BTreeSet<String>, precedence: f64) -> Expr {
        Expr {
            source,
            contexts,
            precedence,
        }
    }

    /// Bare expression text, e.g. `github.ref == 'refs/heads/main'`.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Lowercase names of the top-level contexts referenced by the
    /// expression.
    pub fn contexts(&self) -> &BTreeSet<String> {
        &self.contexts
    }

    /// Operator precedence of the expression, used when composing larger
    /// expressions.
    pub fn precedence(&self) -> f64 {
        self.precedence
    }

    /// Returns the bare expression text without the `${{ ... }}` wrapper.
    pub fn bare(&self) -> String {
        self.source.clone()
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("${{ ")?;
        f.write_str(&self.source)?;
        f.write_str(" }}")
    }
}

impl From<Expr> for Value {
    fn from(expr: Expr) -> Value {
        Value::String(expr.to_string())
    }
}

impl From<&Expr> for Value {
    fn from(expr: &Expr) -> Value {
        Value::String(expr.to_string())
    }
}

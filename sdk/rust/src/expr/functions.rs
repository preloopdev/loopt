//! Built-in GitHub Actions expression functions, ported from the
//! TypeScript SDK's `src/expr/functions.ts`.

use std::collections::BTreeSet;

use super::literal::{operand, Operand};
use super::{precedence, Expr};
use crate::error::{Error, Result};

/// Builds a call expression at Atom precedence. Arguments are rendered
/// through [`operand`] and joined with `", "`; `extra_contexts` records the
/// context-sensitive functions the call depends on (e.g. `hashFiles`'s
/// `hashfiles`, or a status function's own name).
fn call(name: &str, args: &[Operand], extra_contexts: &[&str]) -> Result<Expr> {
    let mut rendered = Vec::with_capacity(args.len());
    for arg in args {
        rendered.push(operand(arg.clone())?);
    }
    let mut contexts: BTreeSet<String> = extra_contexts.iter().map(|s| s.to_lowercase()).collect();
    for expr in &rendered {
        for name in expr.contexts() {
            contexts.insert(name.clone());
        }
    }
    let joined = rendered
        .iter()
        .map(Expr::source)
        .collect::<Vec<_>>()
        .join(", ");
    Ok(Expr::composed(
        format!("{name}({joined})"),
        contexts,
        precedence::ATOM,
    ))
}

/// Builds `contains(search, item)`, which tests whether `search` contains
/// `item` (case-insensitive for strings).
///
/// # Errors
/// Returns [`Error::NonFiniteNumber`] when an argument resolves to a
/// non-finite float literal.
pub fn contains(search: impl Into<Operand>, item: impl Into<Operand>) -> Result<Expr> {
    call("contains", &[search.into(), item.into()], &[])
}

/// Builds `startsWith(value, prefix)`.
///
/// # Errors
/// As [`contains`].
pub fn starts_with(value: impl Into<Operand>, prefix: impl Into<Operand>) -> Result<Expr> {
    call("startsWith", &[value.into(), prefix.into()], &[])
}

/// Builds `endsWith(value, suffix)`.
///
/// # Errors
/// As [`contains`].
pub fn ends_with(value: impl Into<Operand>, suffix: impl Into<Operand>) -> Result<Expr> {
    call("endsWith", &[value.into(), suffix.into()], &[])
}

/// Builds `format(template, ...values)`, substituting each `{N}`
/// placeholder in `template` with the corresponding value.
///
/// # Errors
/// As [`contains`].
pub fn format(template: impl Into<Operand>, values: &[Operand]) -> Result<Expr> {
    let mut args = Vec::with_capacity(values.len() + 1);
    args.push(template.into());
    args.extend_from_slice(values);
    call("format", &args, &[])
}

/// Builds `join(array, separator?)`, which concatenates the array elements
/// with the optional separator (default `","`).
///
/// # Errors
/// As [`contains`].
pub fn join(array: impl Into<Operand>, separator: Option<Operand>) -> Result<Expr> {
    let mut args = vec![array.into()];
    if let Some(separator) = separator {
        args.push(separator);
    }
    call("join", &args, &[])
}

/// Builds `toJSON(value)`, which renders a value as a pretty-printed JSON
/// string.
///
/// # Errors
/// As [`contains`].
pub fn to_json(value: impl Into<Operand>) -> Result<Expr> {
    call("toJSON", &[value.into()], &[])
}

/// Builds `fromJSON(value)`, which parses a JSON string.
///
/// # Errors
/// As [`contains`].
pub fn from_json(value: impl Into<Operand>) -> Result<Expr> {
    call("fromJSON", &[value.into()], &[])
}

/// Builds `hashFiles(...patterns)`, which hashes the files matching the
/// given glob patterns. The result records the `hashfiles`
/// context-sensitive function name.
///
/// # Errors
/// Returns [`Error::InvalidArity`] when called with fewer than 1 or more
/// than 255 patterns.
pub fn hash_files(patterns: &[&str]) -> Result<Expr> {
    if patterns.is_empty() || patterns.len() > 255 {
        return Err(Error::InvalidArity {
            name: "hashFiles",
            requirement: "1 to 255 patterns",
            received: patterns.len(),
        });
    }
    let args: Vec<Operand> = patterns
        .iter()
        .map(|pattern| Operand::from(*pattern))
        .collect();
    call("hashFiles", &args, &["hashfiles"])
}

/// Builds `case(value, predicate, result, ...)`, GitHub's lazy conditional
/// chain. The argument count must be odd so that every predicate has a
/// result and the chain ends with a default.
///
/// # Errors
/// Returns [`Error::InvalidArity`] when called with an even number of
/// arguments or fewer than 3.
pub fn case_of(args: &[Operand]) -> Result<Expr> {
    if args.len() < 3 || args.len().is_multiple_of(2) {
        return Err(Error::InvalidArity {
            name: "case",
            requirement: "an odd number of at least 3 arguments",
            received: args.len(),
        });
    }
    call("case", args, &[])
}

/// Builds a status function call that records its own context name.
fn status(name: &'static str) -> Expr {
    Expr::composed(
        format!("{name}()"),
        [name.to_lowercase()].into_iter().collect(),
        precedence::ATOM,
    )
}

/// Builds `success()`, true when no previous step has failed. Records the
/// `success` context-sensitive function name.
pub fn success() -> Expr {
    status("success")
}

/// Builds `failure()`, true when any previous step has failed. Records the
/// `failure` context-sensitive function name.
pub fn failure() -> Expr {
    status("failure")
}

/// Builds `always()`, which always evaluates to true. Records the `always`
/// context-sensitive function name.
pub fn always() -> Expr {
    status("always")
}

/// Builds `cancelled()`, true when the workflow was cancelled. Records the
/// `cancelled` context-sensitive function name.
pub fn cancelled() -> Expr {
    status("cancelled")
}

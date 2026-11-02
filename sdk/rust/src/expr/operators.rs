//! Comparison, logical, and member-access operators, ported from the
//! TypeScript SDK's `src/expr/operators.ts`.

use std::collections::BTreeSet;

use super::literal::{operand, Operand};
use super::{precedence, Expr};
use crate::error::{Error, Result};

/// Collects the lowercase context names referenced by `exprs`.
fn merge_contexts(exprs: &[&Expr]) -> BTreeSet<String> {
    let mut contexts = BTreeSet::new();
    for expr in exprs {
        for name in expr.contexts() {
            contexts.insert(name.to_lowercase());
        }
    }
    contexts
}

/// Parenthesizes `expr` when it binds less tightly than `limit`.
fn below(expr: Expr, limit: f64) -> Expr {
    if expr.precedence() >= limit {
        return expr;
    }
    let contexts = expr.contexts().clone();
    Expr::composed(format!("({})", expr.source()), contexts, precedence::ATOM)
}

/// Parenthesizes `expr` when it binds no more tightly than `limit`.
fn at_most(expr: Expr, limit: f64) -> Expr {
    if expr.precedence() > limit {
        return expr;
    }
    let contexts = expr.contexts().clone();
    Expr::composed(format!("({})", expr.source()), contexts, precedence::ATOM)
}

/// Builds a binary comparison whose operands are parenthesized at equal
/// precedence.
fn comparison(
    symbol: &str,
    limit: f64,
    left: impl Into<Operand>,
    right: impl Into<Operand>,
) -> Result<Expr> {
    let left = at_most(operand(left)?, limit);
    let right = at_most(operand(right)?, limit);
    let contexts = merge_contexts(&[&left, &right]);
    let source = format!("{} {symbol} {}", left.source(), right.source());
    Ok(Expr::composed(source, contexts, limit))
}

/// Builds an equality comparison `left == right` at Equality precedence.
///
/// # Errors
/// Returns [`Error::NonFiniteNumber`] when `left` or `right` resolves to a
/// non-finite float literal.
pub fn eq(left: impl Into<Operand>, right: impl Into<Operand>) -> Result<Expr> {
    comparison("==", precedence::EQUALITY, left, right)
}

/// Builds an inequality comparison `left != right` at Equality precedence.
///
/// # Errors
/// As [`eq`].
pub fn ne(left: impl Into<Operand>, right: impl Into<Operand>) -> Result<Expr> {
    comparison("!=", precedence::EQUALITY, left, right)
}

/// Builds a less-than comparison `left < right` at Comparison precedence.
///
/// # Errors
/// As [`eq`].
pub fn lt(left: impl Into<Operand>, right: impl Into<Operand>) -> Result<Expr> {
    comparison("<", precedence::COMPARISON, left, right)
}

/// Builds a less-than-or-equal comparison `left <= right` at Comparison
/// precedence.
///
/// # Errors
/// As [`eq`].
pub fn le(left: impl Into<Operand>, right: impl Into<Operand>) -> Result<Expr> {
    comparison("<=", precedence::COMPARISON, left, right)
}

/// Builds a greater-than comparison `left > right` at Comparison
/// precedence.
///
/// # Errors
/// As [`eq`].
pub fn gt(left: impl Into<Operand>, right: impl Into<Operand>) -> Result<Expr> {
    comparison(">", precedence::COMPARISON, left, right)
}

/// Builds a greater-than-or-equal comparison `left >= right` at Comparison
/// precedence.
///
/// # Errors
/// As [`eq`].
pub fn ge(left: impl Into<Operand>, right: impl Into<Operand>) -> Result<Expr> {
    comparison(">=", precedence::COMPARISON, left, right)
}

/// Builds a short-circuit logical chain, flattening operands of equal
/// precedence.
fn logical(name: &'static str, symbol: &str, limit: f64, values: &[Operand]) -> Result<Expr> {
    if values.len() < 2 {
        return Err(Error::InvalidArity {
            name,
            requirement: "at least 2 arguments",
            received: values.len(),
        });
    }
    let mut rendered = Vec::with_capacity(values.len());
    for value in values {
        rendered.push(below(operand(value.clone())?, limit));
    }
    let contexts = merge_contexts(&rendered.iter().collect::<Vec<_>>());
    let joined = rendered
        .iter()
        .map(Expr::source)
        .collect::<Vec<_>>()
        .join(&format!(" {symbol} "));
    Ok(Expr::composed(joined, contexts, limit))
}

/// Builds a short-circuit conjunction `a && b && ...` at And precedence.
///
/// # Errors
/// Returns [`Error::InvalidArity`] when called with fewer than 2 values, or
/// [`Error::NonFiniteNumber`] when a value resolves to a non-finite float
/// literal.
pub fn and(values: &[Operand]) -> Result<Expr> {
    logical("and", "&&", precedence::AND, values)
}

/// Builds a short-circuit disjunction `a || b || ...` at Or precedence.
///
/// # Errors
/// As [`and`].
pub fn or(values: &[Operand]) -> Result<Expr> {
    logical("or", "||", precedence::OR, values)
}

/// Builds a logical negation `!value` at Not precedence.
///
/// # Errors
/// Returns [`Error::NonFiniteNumber`] when `value` resolves to a non-finite
/// float literal.
pub fn not(value: impl Into<Operand>) -> Result<Expr> {
    let inner = below(operand(value)?, precedence::NOT);
    let contexts = inner.contexts().clone();
    let source = format!("!{}", inner.source());
    Ok(Expr::composed(source, contexts, precedence::NOT))
}

/// Builds an index access `object[key]` at Member precedence. The key is
/// rendered through [`operand`], so `at(github, "ref")` produces
/// `github['ref']`.
///
/// # Errors
/// Returns [`Error::NonFiniteNumber`] when `object` or `key` resolves to a
/// non-finite float literal.
pub fn at(object: impl Into<Operand>, key: impl Into<Operand>) -> Result<Expr> {
    let base = below(operand(object)?, precedence::MEMBER);
    let key = operand(key)?;
    let contexts = merge_contexts(&[&base, &key]);
    let source = format!("{}[{}]", base.source(), key.source());
    Ok(Expr::composed(source, contexts, precedence::MEMBER))
}

/// Builds an object-filter access `object.*` at Member precedence.
///
/// # Errors
/// Returns [`Error::NonFiniteNumber`] when `object` resolves to a
/// non-finite float literal.
pub fn star(object: impl Into<Operand>) -> Result<Expr> {
    let base = below(operand(object)?, precedence::MEMBER);
    let contexts = base.contexts().clone();
    let source = format!("{}.*", base.source());
    Ok(Expr::composed(source, contexts, precedence::MEMBER))
}

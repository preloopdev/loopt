//! Literal and operand coercion, ported from the TypeScript SDK's
//! `src/expr/literal.ts`.

use std::collections::BTreeSet;

use super::{precedence, Expr};
use crate::error::{Error, Result};

/// A JSON scalar literal accepted by [`lit`].
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// A string literal, rendered single-quoted with `'` doubled.
    Str(String),
    /// An integer literal, rendered as its decimal text.
    Int(i64),
    /// A floating-point literal; must be finite.
    Float(f64),
    /// `true`/`false`.
    Bool(bool),
    /// `null`.
    Null,
}

impl From<&str> for Literal {
    fn from(value: &str) -> Literal {
        Literal::Str(value.to_string())
    }
}

impl From<String> for Literal {
    fn from(value: String) -> Literal {
        Literal::Str(value)
    }
}

impl From<bool> for Literal {
    fn from(value: bool) -> Literal {
        Literal::Bool(value)
    }
}

impl From<i64> for Literal {
    fn from(value: i64) -> Literal {
        Literal::Int(value)
    }
}

impl From<i32> for Literal {
    fn from(value: i32) -> Literal {
        Literal::Int(i64::from(value))
    }
}

impl From<f64> for Literal {
    fn from(value: f64) -> Literal {
        Literal::Float(value)
    }
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

/// Renders a finite `f64` the way JavaScript's `Number#toString` would (the
/// shortest decimal text that round-trips), matching the TS SDK's `lit()`.
/// Rust's `Display` for `f64` already produces shortest round-trip decimal
/// text and, like JS, omits a trailing `.0` for integer-valued floats
/// (`5.0` renders as `"5"`).
fn format_float(value: f64) -> String {
    format!("{value}")
}

/// Renders `value` as a GitHub Actions literal expression at Atom
/// precedence.
///
/// Strings are single-quoted with embedded single quotes doubled, finite
/// numbers use their shortest round-trip decimal text, booleans render as
/// `true`/`false`, and `null` renders as `null`.
///
/// # Errors
/// Returns [`Error::NonFiniteNumber`] when `value` is a `NaN` or infinite
/// float.
pub fn lit(value: impl Into<Literal>) -> Result<Expr> {
    let source = match value.into() {
        Literal::Str(s) => quote(&s),
        Literal::Int(i) => i.to_string(),
        Literal::Float(f) => {
            if !f.is_finite() {
                return Err(Error::NonFiniteNumber);
            }
            format_float(f)
        }
        Literal::Bool(b) => if b { "true" } else { "false" }.to_string(),
        Literal::Null => "null".to_string(),
    };
    Ok(Expr::composed(source, BTreeSet::new(), precedence::ATOM))
}

/// A value accepted wherever an operator or function expects an operand:
/// either a literal (coerced through [`lit`]) or an already-built [`Expr`].
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    /// A literal, coerced through [`lit`] when resolved.
    Literal(Literal),
    /// An expression, passed through unchanged.
    Expr(Expr),
}

impl From<Literal> for Operand {
    fn from(value: Literal) -> Operand {
        Operand::Literal(value)
    }
}

impl From<Expr> for Operand {
    fn from(value: Expr) -> Operand {
        Operand::Expr(value)
    }
}

impl From<&str> for Operand {
    fn from(value: &str) -> Operand {
        Operand::Literal(value.into())
    }
}

impl From<String> for Operand {
    fn from(value: String) -> Operand {
        Operand::Literal(value.into())
    }
}

impl From<bool> for Operand {
    fn from(value: bool) -> Operand {
        Operand::Literal(value.into())
    }
}

impl From<i64> for Operand {
    fn from(value: i64) -> Operand {
        Operand::Literal(value.into())
    }
}

impl From<i32> for Operand {
    fn from(value: i32) -> Operand {
        Operand::Literal(value.into())
    }
}

impl From<f64> for Operand {
    fn from(value: f64) -> Operand {
        Operand::Literal(value.into())
    }
}

/// Coerces `value` into an expression. Expressions pass through unchanged
/// and literals are wrapped with [`lit`].
///
/// # Errors
/// Returns [`Error::NonFiniteNumber`] when `value` resolves to a `NaN` or
/// infinite float literal.
pub fn operand(value: impl Into<Operand>) -> Result<Expr> {
    match value.into() {
        Operand::Expr(expr) => Ok(expr),
        Operand::Literal(literal) => lit(literal),
    }
}

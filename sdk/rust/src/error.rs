//! The error type shared by the whole SDK.
//!
//! Every failure this crate can raise is one [`Error`] variant, mirroring
//! the stable, machine-checkable failure modes the TypeScript SDK's
//! `LooptError` codes document (`sdk/typescript/src/kit/errors.ts`): an
//! invalid expression source, a non-finite literal or precedence, a
//! variadic call with the wrong argument count, or a reference to an
//! undeclared job output.

use std::fmt;

/// Result alias used throughout `loopt-sdk`, defaulting to this crate's
/// [`Error`] while still allowing a precise override at a call site.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors raised by `loopt-sdk`'s expression and workflow builders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// An expression source was empty or all whitespace.
    EmptyExpression,
    /// An expression source contained `${{` or `}}` outside a single-quoted
    /// string literal.
    ForbiddenDelimiter(&'static str),
    /// An explicit expression precedence was `NaN` or infinite.
    NonFinitePrecedence,
    /// A numeric literal was `NaN` or infinite.
    NonFiniteNumber,
    /// A variadic function (`and`, `or`, `hashFiles`, `case`) was called
    /// with an argument count outside what it accepts.
    InvalidArity {
        /// The function's name, for the error message.
        name: &'static str,
        /// A human-readable description of the accepted count(s), e.g.
        /// `"at least 2 arguments"` or `"1 to 255 patterns"`.
        requirement: &'static str,
        /// The number of arguments actually received.
        received: usize,
    },
    /// A [`crate::core::JobRef::output`] or [`crate::core::StepRef::output`]
    /// call named an output the job or step did not declare.
    UndeclaredOutput {
        /// `"job"` or `"step"`.
        kind: &'static str,
        /// The output name that was requested.
        name: String,
        /// The names actually declared.
        declared: Vec<String>,
    },
    /// [`crate::core::StepRef::output`], `.outcome()`, or `.conclusion()`
    /// was called on a step that was never given an explicit `.id(...)`.
    MissingStepId,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::EmptyExpression => {
                write!(f, "expression source must not be empty or whitespace")
            }
            Error::ForbiddenDelimiter(delimiter) => write!(
                f,
                "expression source must not contain the wrapper delimiter {delimiter:?} outside a single-quoted string literal"
            ),
            Error::NonFinitePrecedence => {
                write!(f, "expression precedence must be a finite number")
            }
            Error::NonFiniteNumber => write!(
                f,
                "cannot render a non-finite number as an expression literal"
            ),
            Error::InvalidArity {
                name,
                requirement,
                received,
            } => write!(f, "{name}() expects {requirement}; received {received}"),
            Error::UndeclaredOutput {
                kind,
                name,
                declared,
            } => {
                let listed = if declared.is_empty() {
                    "none".to_string()
                } else {
                    declared
                        .iter()
                        .map(|output| format!("{output:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                write!(
                    f,
                    "{kind} output {name:?} is not declared; declared outputs: {listed}"
                )
            }
            Error::MissingStepId => write!(
                f,
                "this step has no `id`, so its outputs/outcome/conclusion cannot be referenced"
            ),
        }
    }
}

impl std::error::Error for Error {}

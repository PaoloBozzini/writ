//! Runtime values and errors.

use std::fmt;
use writ_ast::Span;

/// A runtime value. The core language has three ground types; there is no null
/// or nil value, so "non-null by default" holds at runtime by construction.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Text(String),
    /// The value of a statement or a function with no return value.
    Unit,
}

impl Value {
    /// A short name for the value's type, for diagnostics.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Bool(_) => "Bool",
            Value::Text(_) => "Text",
            Value::Unit => "Unit",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{n}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Text(s) => write!(f, "{s}"),
            Value::Unit => write!(f, "()"),
        }
    }
}

/// A runtime error, anchored to the source span that triggered it. The
/// interpreter never panics on ill-typed or ill-behaved programs; it returns
/// one of these instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub message: String,
    pub span: Span,
}

impl RuntimeError {
    #[must_use]
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (at {}..{})",
            self.message, self.span.start, self.span.end
        )
    }
}

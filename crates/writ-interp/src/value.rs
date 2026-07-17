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
    /// For a contract violation, which side is at fault. `None` for ordinary
    /// runtime errors. This is the load-bearing signal a generate-check-repair
    /// loop uses to know which side to regenerate.
    pub blame: Option<Blame>,
}

/// Which side a failed contract blames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blame {
    /// A failed **precondition**: the caller passed bad input.
    Caller,
    /// A failed **postcondition**: the implementation returned a wrong answer.
    Implementation,
}

impl Blame {
    /// The stable wire word for this blame direction.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Blame::Caller => "caller",
            Blame::Implementation => "implementation",
        }
    }
}

impl RuntimeError {
    #[must_use]
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span,
            blame: None,
        }
    }

    /// A failed precondition: blames the caller.
    #[must_use]
    pub fn precondition(span: Span) -> Self {
        Self {
            message: "precondition violated (blame: caller)".to_string(),
            span,
            blame: Some(Blame::Caller),
        }
    }

    /// A failed postcondition: blames the implementation.
    #[must_use]
    pub fn postcondition(span: Span) -> Self {
        Self {
            message: "postcondition violated (blame: implementation)".to_string(),
            span,
            blame: Some(Blame::Implementation),
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

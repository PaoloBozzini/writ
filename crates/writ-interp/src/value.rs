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
    /// A sum-type value: a variant name and its positional payload, e.g.
    /// `Some(3)` or `None`.
    Variant {
        name: String,
        fields: Vec<Value>,
    },
    /// An opaque, unforgeable capability token tagged with the authority it
    /// grants (e.g. `"Root"`, `"Write"`). No surface syntax constructs one; the
    /// runtime hands the root capability to `main`, and `grant` narrows it.
    Capability {
        authority: String,
    },
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
            Value::Variant { .. } => "variant",
            Value::Capability { .. } => "capability",
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
            Value::Capability { authority } => write!(f, "<capability {authority}>"),
            Value::Variant { name, fields } => {
                write!(f, "{name}")?;
                if let Some((first, rest)) = fields.split_first() {
                    write!(f, "({first}")?;
                    for field in rest {
                        write!(f, ", {field}")?;
                    }
                    write!(f, ")")?;
                }
                Ok(())
            }
        }
    }
}

/// A runtime error, anchored to the source span that triggered it. The
/// interpreter never panics on ill-typed or ill-behaved programs; it returns
/// one of these instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    /// Stable code, matching the shared diagnostic schema.
    pub code: String,
    pub message: String,
    pub span: Span,
    /// For a contract violation, which side is at fault. `None` for ordinary
    /// runtime errors. This is the load-bearing signal a generate-check-repair
    /// loop uses to know which side to regenerate.
    pub blame: Option<Blame>,
}

/// Which side a failed contract blames. Re-exported from `writ-ast` so runtime
/// and static diagnostics share one type.
pub use writ_ast::Blame;

impl RuntimeError {
    #[must_use]
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            code: "E1000".to_string(),
            message: message.into(),
            span,
            blame: None,
        }
    }

    /// A failed precondition: blames the caller.
    #[must_use]
    pub fn precondition(span: Span) -> Self {
        Self {
            code: "C0001".to_string(),
            message: "precondition violated (blame: caller)".to_string(),
            span,
            blame: Some(Blame::Caller),
        }
    }

    /// A failed postcondition: blames the implementation.
    #[must_use]
    pub fn postcondition(span: Span) -> Self {
        Self {
            code: "C0002".to_string(),
            message: "postcondition violated (blame: implementation)".to_string(),
            span,
            blame: Some(Blame::Implementation),
        }
    }

    /// Convert to the shared [`writ_ast::Diagnostic`] so runtime errors serialize
    /// under the one machine-readable schema.
    #[must_use]
    pub fn to_diagnostic(&self) -> writ_ast::Diagnostic {
        let d = writ_ast::Diagnostic::error(self.code.clone(), self.span, self.message.clone());
        match self.blame {
            Some(b) => d.with_blame(b),
            None => d,
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

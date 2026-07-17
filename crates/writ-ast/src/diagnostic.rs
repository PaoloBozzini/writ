//! The single shared `Diagnostic` type for the whole compiler.
//!
//! Every pass — lexer, parser, and each checker — reports through this one
//! type. Diagnostics are an API consumed by models, not just prose for humans,
//! so each carries a stable code, an exact span, and a message. The concrete
//! machine-readable serialization format and the parser's stable code set are
//! layered on top in later work; this module fixes the shape everything agrees
//! on.

use crate::span::Span;

/// How severe a diagnostic is. Only errors block compilation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// A problem that prevents the program from being accepted.
    Error,
    /// A concern that does not block compilation.
    Warning,
}

/// A single machine-readable diagnostic.
///
/// The `code` is a stable identifier for the rule that produced the diagnostic
/// (for example `"E0001"`). Stability matters because a generate-check-repair
/// loop keys off it; the human-facing `message` may be reworded without the code
/// changing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    /// Stable identifier for the rule that produced this diagnostic.
    pub code: String,
    /// Exact source location the diagnostic refers to.
    pub span: Span,
    /// Human-facing description. Precise and actionable.
    pub message: String,
    /// Whether this blocks compilation.
    pub severity: Severity,
}

impl Diagnostic {
    /// Construct an error-severity diagnostic.
    #[must_use]
    pub fn error(code: impl Into<String>, span: Span, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            span,
            message: message.into(),
            severity: Severity::Error,
        }
    }

    /// Construct a warning-severity diagnostic.
    #[must_use]
    pub fn warning(code: impl Into<String>, span: Span, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            span,
            message: message.into(),
            severity: Severity::Warning,
        }
    }

    /// Whether this diagnostic blocks compilation.
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_constructor_sets_fields() {
        let d = Diagnostic::error("E0001", Span::new(0, 3), "unexpected token");
        assert_eq!(d.code, "E0001");
        assert_eq!(d.span, Span::new(0, 3));
        assert_eq!(d.message, "unexpected token");
        assert!(d.is_error());
    }

    #[test]
    fn warning_is_not_an_error() {
        let d = Diagnostic::warning("W0001", Span::new(1, 2), "unused binding");
        assert!(!d.is_error());
        assert_eq!(d.severity, Severity::Warning);
    }
}

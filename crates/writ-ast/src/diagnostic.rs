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

impl Severity {
    /// The stable, lowercase wire name used in serialized output.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// Which side a failed contract blames — the load-bearing signal for a
/// generate-check-repair loop. Present only on contract diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Blame {
    /// A failed precondition: the caller passed bad input.
    Caller,
    /// A failed postcondition: the implementation returned a wrong answer.
    Implementation,
}

impl Blame {
    /// The stable, lowercase wire name used in serialized output.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Blame::Caller => "caller",
            Blame::Implementation => "implementation",
        }
    }
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
    /// Blame direction, present only for contract diagnostics.
    pub blame: Option<Blame>,
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
            blame: None,
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
            blame: None,
        }
    }

    /// Attach a blame direction (for contract diagnostics).
    #[must_use]
    pub fn with_blame(mut self, blame: Blame) -> Self {
        self.blame = Some(blame);
        self
    }

    /// Whether this diagnostic blocks compilation.
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Serialize to a single canonical JSON object.
    ///
    /// The format is machine-readable and **deterministic**: fields always
    /// appear in the same order (`code`, `severity`, `span`, `message`) and the
    /// message is escaped so the output is always valid JSON. This is what a
    /// generate-check-repair loop consumes, so stability matters more than
    /// prettiness. Serialization is hand-written to keep `writ-ast` free of
    /// heavy dependencies.
    #[must_use]
    pub fn to_json(&self) -> String {
        let blame = match self.blame {
            Some(b) => format!(",\"blame\":\"{}\"", b.as_str()),
            None => String::new(),
        };
        format!(
            "{{\"code\":\"{}\",\"severity\":\"{}\",\"span\":{{\"start\":{},\"end\":{}}},\"message\":\"{}\"{blame}}}",
            escape_json(&self.code),
            self.severity.as_str(),
            self.span.start,
            self.span.end,
            escape_json(&self.message),
        )
    }
}

/// Serialize a slice of diagnostics to a canonical JSON array, preserving order.
#[must_use]
pub fn diagnostics_to_json(diagnostics: &[Diagnostic]) -> String {
    let mut out = String::from("[");
    for (i, d) in diagnostics.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&d.to_json());
    }
    out.push(']');
    out
}

/// Escape a string so it is a valid JSON string body (without the surrounding
/// quotes). Control characters are emitted as `\u00XX` so the output is pure
/// ASCII and byte-for-byte reproducible.
fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
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

    #[test]
    fn to_json_has_stable_field_order() {
        let d = Diagnostic::error("P0001", Span::new(4, 7), "expected `)`");
        assert_eq!(
            d.to_json(),
            r#"{"code":"P0001","severity":"error","span":{"start":4,"end":7},"message":"expected `)`"}"#
        );
    }

    #[test]
    fn to_json_escapes_message() {
        let d = Diagnostic::error("L0001", Span::new(0, 1), "bad \"quote\"\nand\ttab");
        assert_eq!(
            d.to_json(),
            r#"{"code":"L0001","severity":"error","span":{"start":0,"end":1},"message":"bad \"quote\"\nand\ttab"}"#
        );
    }

    #[test]
    fn blame_is_emitted_only_when_present() {
        let plain = Diagnostic::error("E1000", Span::new(0, 1), "boom");
        assert!(!plain.to_json().contains("blame"));

        let contract = Diagnostic::error("C0002", Span::new(4, 9), "postcondition violated")
            .with_blame(Blame::Implementation);
        assert_eq!(
            contract.to_json(),
            r#"{"code":"C0002","severity":"error","span":{"start":4,"end":9},"message":"postcondition violated","blame":"implementation"}"#
        );
    }

    #[test]
    fn serialization_is_deterministic() {
        let d = Diagnostic::warning("W0007", Span::new(2, 9), "shadowed");
        assert_eq!(d.to_json(), d.to_json());
    }

    #[test]
    fn diagnostics_to_json_is_an_ordered_array() {
        let ds = vec![
            Diagnostic::error("E1", Span::new(0, 1), "a"),
            Diagnostic::error("E2", Span::new(2, 3), "b"),
        ];
        assert_eq!(
            diagnostics_to_json(&ds),
            r#"[{"code":"E1","severity":"error","span":{"start":0,"end":1},"message":"a"},{"code":"E2","severity":"error","span":{"start":2,"end":3},"message":"b"}]"#
        );
        assert_eq!(diagnostics_to_json(&[]), "[]");
    }
}

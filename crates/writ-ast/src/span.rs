//! Source spans: half-open byte ranges into the original source text.

/// A half-open byte range `[start, end)` into the source text.
///
/// Spans are byte offsets, not char or line/column positions, so they are cheap
/// to carry and unambiguous. Every token and every AST node carries one, which
/// is what lets diagnostics anchor to exact source locations — errors are an API
/// for the model, and a precise span is the most load-bearing field in one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// Byte offset of the first byte in the range.
    pub start: usize,
    /// Byte offset one past the last byte in the range.
    pub end: usize,
}

impl Span {
    /// Construct a span from a half-open `[start, end)` byte range.
    ///
    /// `start` must not exceed `end`; a violation is a bug in whatever produced
    /// the span, so it is asserted rather than silently repaired.
    #[must_use]
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end, "span start {start} must not exceed end {end}");
        Self { start, end }
    }

    /// The number of bytes covered by this span.
    #[must_use]
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether this span covers zero bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The smallest span that covers both `self` and `other`.
    #[must_use]
    pub fn merge(&self, other: &Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn len_and_empty() {
        let s = Span::new(3, 7);
        assert_eq!(s.len(), 4);
        assert!(!s.is_empty());
        assert!(Span::new(5, 5).is_empty());
    }

    #[test]
    fn merge_covers_both() {
        let a = Span::new(2, 4);
        let b = Span::new(8, 10);
        assert_eq!(a.merge(&b), Span::new(2, 10));
        assert_eq!(b.merge(&a), Span::new(2, 10));
    }
}

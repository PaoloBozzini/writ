//! `writ-parser` — tokens → AST, via recursive descent.
//!
//! Includes signature parsing for `uses {...}`, `requires`, and `ensures` so
//! the checkers downstream can enforce Writ's two pillars. The parser lands in
//! a later milestone.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {}
}

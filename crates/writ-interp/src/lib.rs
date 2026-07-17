//! `writ-interp` — a tree-walking evaluator over the AST.
//!
//! A back end, not the source of truth: it depends on the AST and never does
//! static analysis. The evaluator lands in a later milestone.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {}
}

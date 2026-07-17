//! `writ-ast` — the shared, standalone data types for the whole compiler.
//!
//! This crate is the contract every other crate agrees on: the AST, plus the
//! `Span` and `Diagnostic` types. It depends on nothing heavy, and nothing in
//! the pipeline may make it depend on the interpreter or the CLI.
//!
//! The concrete node definitions land in later milestones; for now this is the
//! empty scaffold that anchors the workspace's dependency graph.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {}
}

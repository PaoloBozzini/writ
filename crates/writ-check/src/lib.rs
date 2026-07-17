//! `writ-check` — all static analysis over the AST.
//!
//! Home of the two orthogonal pillars: the type checker and effect system, the
//! capability authority checker, and the contract checker. It depends on the
//! AST and **never** on the interpreter. The passes land in later milestones.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {}
}

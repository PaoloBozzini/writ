//! `writ-lexer` — text → tokens, the first stage of the pipeline.
//!
//! Every token carries a byte span (from `writ-ast`) so later stages can
//! anchor deterministic, machine-readable diagnostics to exact source
//! locations. The tokenizer itself lands in a later milestone.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {}
}

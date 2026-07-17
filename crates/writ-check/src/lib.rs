//! `writ-check` — all static analysis over the AST.
//!
//! Home of Writ's checkers. Following the pillar-independence principle, each
//! check is its own pass module with no cross-pass imports: a pass reads the AST
//! and shared [`writ_ast::Diagnostic`]s and nothing else. The passes are
//! composable but independent — either pillar can be run or dropped without the
//! other.
//!
//! Passes are computed transiently and report diagnostics rather than mutating
//! or wrapping the AST, so the AST stays the stable shared contract.

mod effects;
mod ty;
mod types;

pub use effects::check_effects;
pub use ty::Type;
pub use types::check_types;

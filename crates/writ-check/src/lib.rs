//! `writ-check` — all static analysis over the AST.
//!
//! # Pass independence (ARCH-03)
//!
//! Each check is **its own module with no cross-pass imports** — a pass reads
//! the AST and the shared [`writ_ast::Diagnostic`] type and nothing else:
//!
//! | Module | Pass | Reads |
//! | --- | --- | --- |
//! | [`types`] | type checker (+ contract predicate typing) | AST |
//! | [`effects`] | effect inference + honesty | AST |
//! | [`authority`] | capability authority at effect sites | AST (re-derives effect facts) |
//! | [`capabilities`] | `Cap<T>` parameter-only / escape | AST |
//! | [`taint`] | `Tainted<T>` → sink boundary | AST |
//! | [`resolve`] | module qualified names + visibility | AST (module graph) |
//!
//! The two pillars are **orthogonal**: the type/contract checking imports none
//! of the capability, effect, or taint passes, so either pillar can be deleted
//! and the other still compiles and runs. The one conceptual cross-pass
//! dependency — authority needing effect facts — is satisfied by `authority`
//! re-deriving them from signatures, not by importing `effects`. Any subset of
//! passes can be run independently (see the CLI's `writ check <file> [pass...]`).
//!
//! Passes are computed transiently and report diagnostics rather than mutating
//! or wrapping the AST, so the AST stays the stable shared contract.

mod authority;
mod builtins;
mod capabilities;
mod effects;
mod resolve;
mod taint;
mod ty;
mod types;

pub use authority::check_authority;
pub use capabilities::check_capabilities;
pub use effects::check_effects;
pub use resolve::check_resolution;
pub use taint::check_taint;
pub use ty::Type;
pub use types::check_types;

//! `writ-ast` — the shared, standalone data types for the whole compiler.
//!
//! This crate is the contract every other crate agrees on: the abstract syntax
//! tree, plus the [`Span`] and [`Diagnostic`] types. It depends on nothing
//! heavy, and nothing in the pipeline may make it depend on the interpreter or
//! the CLI.
//!
//! The AST is deliberately *untyped* — nodes carry spans but no pass results —
//! so it can serve both the interpreter and the future compiler unchanged.

mod ast;
mod diagnostic;
mod span;

pub use ast::{
    BinaryOp, Block, Contract, Effect, EffectSet, Expr, Function, Item, Literal, LiteralKind,
    Module, Param, Signature, Stmt, TypeExpr, UnaryOp,
};
pub use diagnostic::{Diagnostic, Severity};
pub use span::Span;

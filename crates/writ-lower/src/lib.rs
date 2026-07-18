//! Contract **desugaring** — the one place where `requires` / `ensures`
//! semantics are turned into executable checks.
//!
//! The prime motivation (issue #38): runtime contract checking with blame must
//! behave **identically** in the tree-walking interpreter and the native back
//! end. If each back end re-implemented `requires` / `ensures`, the two would
//! drift and the interpreter would silently become the de-facto spec. Instead
//! this pass lowers every contract clause **once** into the shared
//! [`Stmt::Check`] form (a runtime assertion carrying blame). Every back end
//! then implements only `Check` — a single, small, shared semantics.
//!
//! Per the recorded decision on #38, the lowering is a **rewrite over the AST
//! we already have** — no separate IR. It consumes a [`Module`] and produces a
//! [`Module`]:
//!
//! - each `requires` clause becomes a [`Stmt::Check`] with [`Blame::Caller`],
//!   prepended to the body (checked on entry, against the arguments);
//! - each `ensures` clause becomes a [`Stmt::Check`] with
//!   [`Blame::Implementation`], run on **every** exit path with `result` bound
//!   to the returned value;
//! - the signature's `requires` / `ensures` are then cleared, so the lowering
//!   is **idempotent**: lowering an already-lowered module is a no-op.
//!
//! This crate depends only on `writ-ast`, so it sits below every back end in the
//! dependency graph and never pulls a checker or an evaluator in with it.

use writ_ast::{Blame, Block, Expr, Function, Item, Module, Stmt};

/// Desugar every function's contracts into [`Stmt::Check`] statements, returning
/// a new module. Non-function items and imports are preserved unchanged.
#[must_use]
pub fn lower(module: &Module) -> Module {
    Module {
        imports: module.imports.clone(),
        items: module
            .items
            .iter()
            .map(|item| match item {
                Item::Function(f) => Item::Function(lower_function(f)),
                Item::Type(t) => Item::Type(t.clone()),
            })
            .collect(),
    }
}

fn lower_function(f: &Function) -> Function {
    let sig = &f.signature;

    // `ensures` clauses become implementation-blamed checks, run at every exit.
    let ensures: Vec<Stmt> = sig
        .ensures
        .iter()
        .map(|c| Stmt::Check {
            predicate: c.predicate.clone(),
            blame: Blame::Implementation,
            span: c.span,
        })
        .collect();

    let mut stmts: Vec<Stmt> = Vec::new();

    // `requires` clauses become caller-blamed checks, on entry against the args.
    for c in &sig.requires {
        stmts.push(Stmt::Check {
            predicate: c.predicate.clone(),
            blame: Blame::Caller,
            span: c.span,
        });
    }

    // Rewrite the body so `ensures` runs on every `return`.
    let body = rewrite_stmts(&f.body.stmts, &ensures);
    let ends_in_return = matches!(body.last(), Some(Stmt::Return { .. }));
    stmts.extend(body);

    // A path that falls off the end (a `Unit`-returning function, or one that
    // returns only inside a conditional) must still run `ensures`. Appending
    // when the body does not unconditionally end in a `return` covers that path;
    // where every path already returned, the appended checks are unreachable.
    if !ensures.is_empty() && !ends_in_return {
        stmts.extend(ensures.iter().cloned());
    }

    // Clear the now-lowered clauses so the pass is idempotent.
    let mut new_sig = sig.clone();
    new_sig.requires = Vec::new();
    new_sig.ensures = Vec::new();

    Function {
        exported: f.exported,
        signature: new_sig,
        body: Block {
            stmts,
            span: f.body.span,
        },
        span: f.span,
    }
}

/// Rewrite a statement sequence so each `return` runs the `ensures` checks with
/// `result` bound to the returned value. With no `ensures`, statements are
/// returned unchanged. Recurses into `if` branches, the only other place a
/// `return` can appear.
fn rewrite_stmts(stmts: &[Stmt], ensures: &[Stmt]) -> Vec<Stmt> {
    if ensures.is_empty() {
        return stmts.to_vec();
    }
    let mut out = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::Return {
                value: Some(expr),
                span,
            } => {
                // let result = <expr>; <ensures>; return result;
                out.push(Stmt::Let {
                    name: "result".to_string(),
                    mutable: false,
                    ty: None,
                    value: expr.clone(),
                    span: *span,
                });
                out.extend(ensures.iter().cloned());
                out.push(Stmt::Return {
                    value: Some(Expr::Identifier {
                        name: "result".to_string(),
                        span: *span,
                    }),
                    span: *span,
                });
            }
            Stmt::Return { value: None, span } => {
                // A valueless return: run `ensures` (any reference to `result`
                // would be `Unit`), then return.
                out.extend(ensures.iter().cloned());
                out.push(Stmt::Return {
                    value: None,
                    span: *span,
                });
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                span,
            } => {
                out.push(Stmt::If {
                    cond: cond.clone(),
                    then_block: Block {
                        stmts: rewrite_stmts(&then_block.stmts, ensures),
                        span: then_block.span,
                    },
                    else_block: else_block.as_ref().map(|b| Block {
                        stmts: rewrite_stmts(&b.stmts, ensures),
                        span: b.span,
                    }),
                    span: *span,
                });
            }
            other => out.push(other.clone()),
        }
    }
    out
}

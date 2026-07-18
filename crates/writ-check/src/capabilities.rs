//! The capability pass: `Cap<T>` tokens are parameter-only and second-class.
//!
//! A capability represents unforgeable authority (file write, network, …). Writ
//! has **no ambient authority**: a capability cannot be constructed in user code
//! — there is no literal, constructor, or built-in that yields one — so the only
//! way a `Cap<T>` value can exist is by being passed in as a function parameter.
//! A function with no capability parameter is therefore sandboxed *by
//! construction*.
//!
//! Capabilities are also **second-class** (see the spec's escape-semantics
//! decision): a capability may only be received as a parameter and forwarded as
//! a call argument. It may not escape upward or be stashed, so this pass rejects:
//!
//! - returning a capability (`E0201`), and
//! - binding a capability to a local (`E0202`).
//!
//! Those are the only escape channels the language currently has (no closures,
//! structs, or collections), so forbidding them preserves the invariant "a
//! function with no capability parameter can reach no effects".
//!
//! This pass is self-contained and imports no other checker.

use std::collections::HashSet;

use writ_ast::{Block, Diagnostic, Expr, Item, Module, Stmt, TypeExpr};

/// The type-head that marks a capability type, e.g. `Cap<Write>`.
const CAP: &str = "Cap";

/// Whether a syntactic type is a capability type.
fn is_cap(ty: &TypeExpr) -> bool {
    ty.name == CAP
}

/// Check the capability rules over a module. Empty result means every
/// capability is parameter-only and never escapes.
#[must_use]
pub fn check_capabilities(module: &Module) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in &module.items {
        let Item::Function(f) = item else {
            continue;
        };
        let sig = &f.signature;

        // The names of this function's capability parameters — the only
        // capability values in scope.
        let cap_params: HashSet<&str> = sig
            .params
            .iter()
            .filter(|p| is_cap(&p.ty))
            .map(|p| p.name.as_str())
            .collect();

        // A capability must not escape upward via the return type.
        if let Some(rt) = &sig.return_type {
            if is_cap(rt) {
                diagnostics.push(Diagnostic::error(
                    "E0201",
                    rt.span,
                    "a capability cannot be returned: `Cap<..>` is second-class (authority only flows downward, through parameters)",
                ));
            }
        }

        check_block(&f.body, &cap_params, &mut diagnostics);
    }
    diagnostics
}

fn check_block(block: &Block, cap_params: &HashSet<&str>, out: &mut Vec<Diagnostic>) {
    for stmt in &block.stmts {
        check_stmt(stmt, cap_params, out);
    }
}

fn check_stmt(stmt: &Stmt, cap_params: &HashSet<&str>, out: &mut Vec<Diagnostic>) {
    match stmt {
        Stmt::Let {
            name,
            ty,
            value,
            span,
            ..
        } => {
            // A capability cannot be stashed in a local, whether by annotation
            // or by binding a capability parameter. Since a capability can only
            // originate from a parameter and this rule forbids re-binding one,
            // no local ever holds a capability.
            let annotated_cap = ty.as_ref().is_some_and(is_cap);
            let binds_cap = is_capability_ref(value, cap_params);
            if annotated_cap || binds_cap {
                out.push(Diagnostic::error(
                    "E0202",
                    *span,
                    format!(
                        "capability `{name}` cannot be bound to a local: capabilities enter scope only as parameters and are passed on directly as arguments"
                    ),
                ));
            }
        }
        Stmt::Return {
            value: Some(expr),
            span,
        } => {
            if is_capability_ref(expr, cap_params) {
                out.push(Diagnostic::error(
                    "E0201",
                    *span,
                    "a capability cannot be returned: `Cap<..>` is second-class",
                ));
            }
        }
        // A lowered contract predicate is a pure boolean expression: it cannot
        // bind, return, or otherwise let a capability escape.
        Stmt::Return { value: None, .. } | Stmt::Expr(_) | Stmt::Check { .. } => {}
        Stmt::If {
            then_block,
            else_block,
            ..
        } => {
            check_block(then_block, cap_params, out);
            if let Some(else_block) = else_block {
                check_block(else_block, cap_params, out);
            }
        }
    }
}

/// Whether `expr` directly names a capability parameter (the only expression
/// that can have capability type — capabilities are otherwise unconstructible).
fn is_capability_ref(expr: &Expr, cap_params: &HashSet<&str>) -> bool {
    matches!(expr, Expr::Identifier { name, .. } if cap_params.contains(name.as_str()))
}

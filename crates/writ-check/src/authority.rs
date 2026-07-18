//! The authority pass: at every effect site, the caller must hold a matching
//! capability.
//!
//! This is the authority half of the effect-site rule, paired with the honesty
//! check (`effects`). Honesty says a signature must *declare* every effect its
//! body performs; authority says the body may only perform an effect it holds a
//! **capability** for. Together: an effect is reachable only when the signature
//! declared it *and* an unforgeable `Cap<E>` token was passed in — so dangerous
//! power is unreachable by default.
//!
//! A function **holds** effect `E` iff it has a parameter of type `Cap<E>`. An
//! **effect site** is a call to a function that declares `uses {E}`. If the
//! caller does not hold `Cap<E>` at such a site, it is rejected before the
//! program runs (`E0301`).
//!
//! This pass consumes only signatures — it re-derives the effect facts it needs
//! rather than importing another checker, so the passes stay independent.

use std::collections::{HashMap, HashSet};

use writ_ast::{Block, Diagnostic, Expr, Item, Module, Stmt, TypeExpr};

/// The type-head marking a capability type, e.g. `Cap<Write>`.
const CAP: &str = "Cap";

/// The authority of the root capability — holds every effect.
const ROOT: &str = "Root";

/// The authority a `Cap<E>` parameter grants: the inner effect name `E`.
fn granted_effect(ty: &TypeExpr) -> Option<&str> {
    if ty.name == CAP {
        ty.args.first().map(|a| a.name.as_str())
    } else {
        None
    }
}

/// Check that every effect site holds a matching capability. Empty result means
/// no function performs an effect it lacks the capability for.
#[must_use]
pub fn check_authority(module: &Module) -> Vec<Diagnostic> {
    // Each function's declared effects, in source order.
    let declared: HashMap<&str, Vec<&str>> = module
        .items
        .iter()
        .filter_map(|item| {
            let Item::Function(f) = item else {
                return None;
            };
            let effects = f
                .signature
                .effects
                .effects
                .iter()
                .map(|e| e.name.as_str())
                .collect();
            Some((f.signature.name.as_str(), effects))
        })
        .collect();

    let mut diagnostics = Vec::new();
    for item in &module.items {
        let Item::Function(f) = item else {
            continue;
        };
        // The effects this function holds a capability for.
        let held: HashSet<&str> = f
            .signature
            .params
            .iter()
            .filter_map(|p| granted_effect(&p.ty))
            .collect();
        // The root capability holds every authority: `main` receives it and
        // narrows it downward via `grant`, so a holder is authorized for any
        // effect site.
        if held.contains(ROOT) {
            continue;
        }
        let fn_name = f.signature.name.as_str();

        let mut calls = Vec::new();
        collect_calls_in_block(&f.body, &mut calls);

        for call in calls {
            let Expr::Call { callee, span, .. } = call else {
                continue;
            };
            let Expr::Identifier {
                name: callee_name, ..
            } = callee.as_ref()
            else {
                continue;
            };
            let Some(effects) = declared.get(callee_name.as_str()) else {
                continue;
            };
            for effect in effects {
                if !held.contains(effect) {
                    diagnostics.push(Diagnostic::error(
                        "E0301",
                        *span,
                        format!(
                            "effect `{effect}` is performed here (via call to `{callee_name}`) but function `{fn_name}` holds no `Cap<{effect}>` capability"
                        ),
                    ));
                }
            }
        }
    }
    diagnostics
}

/// Collect every call expression in a block, in source order. Kept local so the
/// pass imports no other checker.
fn collect_calls_in_block<'a>(block: &'a Block, out: &mut Vec<&'a Expr>) {
    for stmt in &block.stmts {
        collect_calls_in_stmt(stmt, out);
    }
}

fn collect_calls_in_stmt<'a>(stmt: &'a Stmt, out: &mut Vec<&'a Expr>) {
    match stmt {
        Stmt::Let { value, .. } => collect_calls_in_expr(value, out),
        Stmt::Expr(e) => collect_calls_in_expr(e, out),
        Stmt::Return { value: Some(e), .. } => collect_calls_in_expr(e, out),
        Stmt::Return { value: None, .. } => {}
        Stmt::Check { predicate, .. } => collect_calls_in_expr(predicate, out),
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            collect_calls_in_expr(cond, out);
            collect_calls_in_block(then_block, out);
            if let Some(else_block) = else_block {
                collect_calls_in_block(else_block, out);
            }
        }
    }
}

fn collect_calls_in_expr<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match expr {
        Expr::Call { callee, args, .. } => {
            out.push(expr);
            collect_calls_in_expr(callee, out);
            for arg in args {
                collect_calls_in_expr(arg, out);
            }
        }
        Expr::Unary { operand, .. } => collect_calls_in_expr(operand, out),
        Expr::Binary { left, right, .. } => {
            collect_calls_in_expr(left, out);
            collect_calls_in_expr(right, out);
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_calls_in_expr(scrutinee, out);
            for arm in arms {
                collect_calls_in_expr(&arm.body, out);
            }
        }
        Expr::Member { base, .. } => collect_calls_in_expr(base, out),
        Expr::Literal(_) | Expr::Identifier { .. } => {}
    }
}

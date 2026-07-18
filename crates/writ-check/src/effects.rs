//! The effect-inference and effect-subset pass.
//!
//! Every function's signature declares its effect set via `uses {...}`. That
//! declared set is taken as the authority a *call* to the function costs — the
//! leaf effectful operations of the language bottom out in functions that
//! declare an effect. A body therefore performs the union of the effects
//! declared by the functions it calls, and this pass verifies that union is a
//! subset of what the calling function itself declared.
//!
//! Effects flow through calls transitively: because each function is checked
//! against its own declaration, a caller only needs to look at the *declared*
//! effects of its direct callees to see everything reachable beneath them.
//!
//! This pass is self-contained and imports no other checker.

use std::collections::{HashMap, HashSet};

use writ_ast::{Block, Diagnostic, Expr, Item, Module, Stmt};

/// Verify every function's inferred effects are a subset of its declared
/// `uses {...}` set. Returns diagnostics in source order; empty means every
/// function honestly declares at least the effects its body performs.
#[must_use]
pub fn check_effects(module: &Module) -> Vec<Diagnostic> {
    // Each function's declared effects, in source order (deterministic output).
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
        let declared_here: HashSet<&str> = f
            .signature
            .effects
            .effects
            .iter()
            .map(|e| e.name.as_str())
            .collect();

        let fn_name = f.signature.name.as_str();

        // Contract predicates must be **pure**: the interpreter evaluates
        // `requires` / `ensures` at runtime, so an effectful call inside one is
        // an effect the signature never declares. Rather than treat predicates
        // as effect sites, the language forbids effects in them outright —
        // contracts assert correctness, they must not *do* anything.
        let mut predicate_calls = Vec::new();
        for clause in f.signature.requires.iter().chain(&f.signature.ensures) {
            collect_calls_in_expr(&clause.predicate, &mut predicate_calls);
        }
        for call in &predicate_calls {
            let Expr::Call { callee, span, .. } = call else {
                continue;
            };
            let Expr::Identifier { name, .. } = callee.as_ref() else {
                continue;
            };
            if declared.get(name.as_str()).is_some_and(|e| !e.is_empty()) {
                diagnostics.push(Diagnostic::error(
                    "E0102",
                    *span,
                    format!(
                        "contract predicate of `{fn_name}` must be pure, but it calls effectful function `{name}`"
                    ),
                ));
            }
        }

        let mut calls = Vec::new();
        collect_calls_in_block(&f.body, &mut calls);

        for call in calls {
            let Expr::Call { callee, span, .. } = call else {
                continue;
            };
            let Expr::Identifier { name, .. } = callee.as_ref() else {
                continue;
            };
            let Some(callee_effects) = declared.get(name.as_str()) else {
                continue;
            };
            for effect in callee_effects {
                if !declared_here.contains(effect) {
                    // The honesty diagnostic names all three things a repair loop
                    // needs: the effect, the effect site (this call, attributed to
                    // the callee it flowed through), and the signature that
                    // omitted it. A signature can never under-report its power.
                    diagnostics.push(Diagnostic::error(
                        "E0101",
                        *span,
                        format!(
                            "function `{fn_name}` performs effect `{effect}` here (via call to `{name}`) but its signature's `uses {{...}}` set omits it"
                        ),
                    ));
                }
            }
        }
    }
    diagnostics
}

/// Collect every call expression in a block, in source order (pre-order).
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
            // Record this call before descending, so outer effects are reported
            // before inner ones.
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

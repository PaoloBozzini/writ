//! Translate a function's contracts into SMT-LIB2 verification queries.
//!
//! For each `ensures` clause we build a query that is **unsatisfiable exactly
//! when the postcondition holds for all inputs**: assume the `requires`, model
//! the body's `result`, and assert the *negation* of the `ensures`. A solver
//! answering `unsat` has proved the clause; `sat` exhibits a counterexample.
//!
//! ## Supported fragment
//! Deliberately small and **sound** (issue #27, scoped to quantifier-free
//! integer arithmetic on pure, non-recursive functions):
//! - parameters of type `Int` or `Bool`;
//! - bodies of `let` / `if` / `return` over `+ - *`, comparisons, boolean
//!   operators, equality, and unary `-` / `!`;
//! - **no** `/` or `%` (SMT integer division rounds differently from Writ's
//!   truncation, so including them would be unsound), no calls, no `match`, no
//!   `Text`.
//!
//! A function outside this fragment yields no queries — it is reported as "not
//! statically verified", never silently assumed correct. Integers are modelled
//! as unbounded ℤ (overflow, which traps at runtime, is not modelled).

use std::collections::HashMap;

use writ_ast::{BinaryOp, Expr, Function, LiteralKind, Span, Stmt, UnaryOp};

/// One verification query: the SMT-LIB2 script and the `ensures` clause it
/// proves (so a failure can be blamed on that clause's span).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub clause_span: Span,
    pub script: String,
}

/// Build the verification queries for `f` — one per `ensures` clause that lies
/// within the supported fragment. Returns an empty vec if the function has no
/// `ensures`, or if its parameters/body/contracts fall outside the fragment.
#[must_use]
pub fn function_queries(f: &Function) -> Vec<Query> {
    let sig = &f.signature;
    if sig.ensures.is_empty() {
        return Vec::new();
    }

    // Parameters must be Int or Bool; anything else leaves the fragment.
    let mut decls = String::new();
    let mut env: HashMap<String, String> = HashMap::new();
    for p in &sig.params {
        let sort = match p.ty.name.as_str() {
            "Int" if p.ty.args.is_empty() => "Int",
            "Bool" if p.ty.args.is_empty() => "Bool",
            _ => return Vec::new(),
        };
        decls.push_str(&format!("(declare-const {} {sort})\n", p.name));
        env.insert(p.name.clone(), p.name.clone());
    }

    // Model the body's result symbolically.
    let Some(result) = model_stmts(&f.body.stmts, &mut env.clone()) else {
        return Vec::new();
    };

    // Assumptions: every `requires` must translate, or we cannot soundly assume
    // the rest, so we bail out of the whole function.
    let mut assumptions = String::new();
    for c in &sig.requires {
        let Some(pred) = translate(&c.predicate, &env) else {
            return Vec::new();
        };
        assumptions.push_str(&format!("(assert {pred})\n"));
    }

    // One query per ensures clause, with `result` bound to the modelled value.
    let mut env_ens = env.clone();
    env_ens.insert("result".to_string(), result);
    let mut queries = Vec::new();
    for c in &sig.ensures {
        let Some(pred) = translate(&c.predicate, &env_ens) else {
            // This clause is outside the fragment; skip it (reported upstream).
            continue;
        };
        let script = format!("{decls}{assumptions}(assert (not {pred}))\n(check-sat)\n");
        queries.push(Query {
            clause_span: c.span,
            script,
        });
    }
    queries
}

/// Model a statement sequence's `result` as an SMT term, threading `let`
/// bindings through `env`. Returns `None` for anything outside the fragment.
fn model_stmts(stmts: &[Stmt], env: &mut HashMap<String, String>) -> Option<String> {
    let (head, tail) = stmts.split_first()?;
    match head {
        Stmt::Let { name, value, .. } => {
            let v = translate(value, env)?;
            env.insert(name.clone(), v);
            model_stmts(tail, env)
        }
        Stmt::Return { value: Some(e), .. } => translate(e, env),
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            let c = translate(cond, env)?;
            let t = model_stmts(&then_block.stmts, &mut env.clone())?;
            // With no explicit `else`, the false path is the continuation.
            let e = match else_block {
                Some(b) => model_stmts(&b.stmts, &mut env.clone())?,
                None => model_stmts(tail, &mut env.clone())?,
            };
            Some(format!("(ite {c} {t} {e})"))
        }
        // A valueless return or a bare expression contributes no result; a
        // `Check` never appears here (verification runs on the un-lowered AST).
        Stmt::Return { value: None, .. } | Stmt::Expr(_) | Stmt::Check { .. } => {
            model_stmts(tail, env)
        }
    }
}

/// Translate an expression to an SMT-LIB2 term, or `None` if it uses a construct
/// outside the supported fragment.
fn translate(expr: &Expr, env: &HashMap<String, String>) -> Option<String> {
    match expr {
        Expr::Literal(lit) => match &lit.kind {
            LiteralKind::Int(n) => Some(if *n < 0 {
                format!("(- {})", n.unsigned_abs())
            } else {
                n.to_string()
            }),
            LiteralKind::Bool(b) => Some(b.to_string()),
            LiteralKind::Text(_) => None,
        },
        Expr::Identifier { name, .. } => env.get(name).cloned(),
        Expr::Unary { op, operand, .. } => {
            let x = translate(operand, env)?;
            Some(match op {
                UnaryOp::Neg => format!("(- {x})"),
                UnaryOp::Not => format!("(not {x})"),
            })
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            let l = translate(left, env)?;
            let r = translate(right, env)?;
            let sym = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
                BinaryOp::Eq => "=",
                BinaryOp::Ne => "distinct",
                BinaryOp::And => "and",
                BinaryOp::Or => "or",
                // Division/remainder are excluded: SMT integer division does not
                // match Writ's truncation, so modelling them would be unsound.
                BinaryOp::Div | BinaryOp::Rem => return None,
            };
            Some(format!("({sym} {l} {r})"))
        }
        // Calls, `match`, and member access are outside the fragment.
        Expr::Call { .. } | Expr::Match { .. } | Expr::Member { .. } => None,
    }
}

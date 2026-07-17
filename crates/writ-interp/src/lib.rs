//! `writ-interp` — a tree-walking evaluator over the AST.
//!
//! A back end, not the source of truth: it evaluates the AST that `writ-parser`
//! produces and never performs static analysis. It also never panics on a bad
//! program — every arithmetic overflow, division by zero, or type mismatch
//! becomes a [`RuntimeError`] anchored to a span.
//!
//! It evaluates the core expression forms (literals and the arithmetic,
//! comparison, and logical operators), `let` bindings with lexical scope, and
//! blocks with `return`. Because it walks the tree the parser built, operator
//! precedence is already baked into the shape, so `1 + 2 * 3` evaluates to `7`.
//! Function calls arrive in later work.

mod env;
mod value;

use writ_ast::{BinaryOp, Block, Expr, Literal, LiteralKind, Stmt, UnaryOp};

pub use env::Env;
pub use value::{RuntimeError, Value};

/// How evaluation of a statement flows: continue with a value, or return out of
/// the enclosing block carrying a value.
enum Control {
    Normal(Value),
    Return(Value),
}

/// Evaluate a core expression to a [`Value`] in an empty environment.
///
/// # Errors
/// Returns a [`RuntimeError`] on overflow, division by zero, a type mismatch, an
/// unbound variable, or a call (calls arrive in later work).
pub fn eval_expr(expr: &Expr) -> Result<Value, RuntimeError> {
    let mut env = Env::new();
    eval_expr_in(expr, &mut env)
}

/// Evaluate a block of statements in a fresh environment, returning the block's
/// value (the value of a `return`, else `Unit`).
///
/// # Errors
/// Returns a [`RuntimeError`] on any evaluation failure inside the block.
pub fn eval_block(block: &Block) -> Result<Value, RuntimeError> {
    let mut env = Env::new();
    eval_block_in(block, &mut env)
}

/// Evaluate a block in the given environment, opening a nested lexical scope for
/// the duration so its bindings do not leak to the caller.
fn eval_block_in(block: &Block, env: &mut Env) -> Result<Value, RuntimeError> {
    env.push_scope();
    let result = run_stmts(&block.stmts, env);
    env.pop_scope();
    match result? {
        Control::Normal(v) | Control::Return(v) => Ok(v),
    }
}

/// Run a sequence of statements, stopping early on a `return`.
fn run_stmts(stmts: &[Stmt], env: &mut Env) -> Result<Control, RuntimeError> {
    let mut last = Value::Unit;
    for stmt in stmts {
        match eval_stmt(stmt, env)? {
            Control::Normal(v) => last = v,
            ret @ Control::Return(_) => return Ok(ret),
        }
    }
    Ok(Control::Normal(last))
}

fn eval_stmt(stmt: &Stmt, env: &mut Env) -> Result<Control, RuntimeError> {
    match stmt {
        Stmt::Let {
            name,
            mutable,
            value,
            span,
            ..
        } => {
            let v = eval_expr_in(value, env)?;
            env.define(name, v, *mutable)
                .map_err(|msg| RuntimeError::new(*span, msg))?;
            Ok(Control::Normal(Value::Unit))
        }
        Stmt::Expr(expr) => Ok(Control::Normal(eval_expr_in(expr, env)?)),
        Stmt::Return { value, .. } => {
            let v = match value {
                Some(expr) => eval_expr_in(expr, env)?,
                None => Value::Unit,
            };
            Ok(Control::Return(v))
        }
    }
}

/// Evaluate an expression in the given environment.
fn eval_expr_in(expr: &Expr, env: &mut Env) -> Result<Value, RuntimeError> {
    match expr {
        Expr::Literal(lit) => Ok(eval_literal(lit)),
        Expr::Unary { op, operand, span } => {
            let v = eval_expr_in(operand, env)?;
            eval_unary(*op, v, *span)
        }
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => eval_binary(*op, left, right, *span, env),
        Expr::Identifier { name, span } => env
            .lookup(name)
            .ok_or_else(|| RuntimeError::new(*span, format!("unbound variable `{name}`"))),
        Expr::Call { span, .. } => Err(RuntimeError::new(
            *span,
            "function calls are not supported by the evaluator yet",
        )),
    }
}

fn eval_literal(lit: &Literal) -> Value {
    match &lit.kind {
        LiteralKind::Int(n) => Value::Int(*n),
        LiteralKind::Bool(b) => Value::Bool(*b),
        LiteralKind::Text(s) => Value::Text(s.clone()),
    }
}

fn eval_unary(op: UnaryOp, v: Value, span: writ_ast::Span) -> Result<Value, RuntimeError> {
    match (op, v) {
        (UnaryOp::Neg, Value::Int(n)) => n
            .checked_neg()
            .map(Value::Int)
            .ok_or_else(|| RuntimeError::new(span, "integer overflow negating value")),
        (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnaryOp::Neg, other) => Err(RuntimeError::new(
            span,
            format!("cannot negate a value of type {}", other.type_name()),
        )),
        (UnaryOp::Not, other) => Err(RuntimeError::new(
            span,
            format!("cannot apply `!` to a value of type {}", other.type_name()),
        )),
    }
}

fn eval_binary(
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
    span: writ_ast::Span,
    env: &mut Env,
) -> Result<Value, RuntimeError> {
    // Logical operators short-circuit, so evaluate the right side lazily.
    match op {
        BinaryOp::And => {
            return match eval_bool(left, env)? {
                false => Ok(Value::Bool(false)),
                true => Ok(Value::Bool(eval_bool(right, env)?)),
            };
        }
        BinaryOp::Or => {
            return match eval_bool(left, env)? {
                true => Ok(Value::Bool(true)),
                false => Ok(Value::Bool(eval_bool(right, env)?)),
            };
        }
        _ => {}
    }

    let l = eval_expr_in(left, env)?;
    let r = eval_expr_in(right, env)?;
    match op {
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
            arithmetic(op, l, r, span)
        }
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => compare(op, l, r, span),
        BinaryOp::Eq => Ok(Value::Bool(equal(&l, &r, span)?)),
        BinaryOp::Ne => Ok(Value::Bool(!equal(&l, &r, span)?)),
        BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),
    }
}

fn arithmetic(
    op: BinaryOp,
    l: Value,
    r: Value,
    span: writ_ast::Span,
) -> Result<Value, RuntimeError> {
    let (Value::Int(a), Value::Int(b)) = (&l, &r) else {
        return Err(RuntimeError::new(
            span,
            format!(
                "arithmetic requires two Int operands, found {} and {}",
                l.type_name(),
                r.type_name()
            ),
        ));
    };
    let (a, b) = (*a, *b);
    let result = match op {
        BinaryOp::Add => a.checked_add(b),
        BinaryOp::Sub => a.checked_sub(b),
        BinaryOp::Mul => a.checked_mul(b),
        BinaryOp::Div => {
            if b == 0 {
                return Err(RuntimeError::new(span, "division by zero"));
            }
            a.checked_div(b)
        }
        BinaryOp::Rem => {
            if b == 0 {
                return Err(RuntimeError::new(span, "remainder by zero"));
            }
            a.checked_rem(b)
        }
        _ => unreachable!(),
    };
    result
        .map(Value::Int)
        .ok_or_else(|| RuntimeError::new(span, "integer overflow"))
}

fn compare(op: BinaryOp, l: Value, r: Value, span: writ_ast::Span) -> Result<Value, RuntimeError> {
    let (Value::Int(a), Value::Int(b)) = (&l, &r) else {
        return Err(RuntimeError::new(
            span,
            format!(
                "comparison requires two Int operands, found {} and {}",
                l.type_name(),
                r.type_name()
            ),
        ));
    };
    let (a, b) = (*a, *b);
    Ok(Value::Bool(match op {
        BinaryOp::Lt => a < b,
        BinaryOp::Le => a <= b,
        BinaryOp::Gt => a > b,
        BinaryOp::Ge => a >= b,
        _ => unreachable!(),
    }))
}

/// Structural equality within a single type. Comparing different types is a
/// type error (no implicit coercion), reported rather than silently `false`.
fn equal(l: &Value, r: &Value, span: writ_ast::Span) -> Result<bool, RuntimeError> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Ok(a == b),
        (Value::Bool(a), Value::Bool(b)) => Ok(a == b),
        (Value::Text(a), Value::Text(b)) => Ok(a == b),
        _ => Err(RuntimeError::new(
            span,
            format!("cannot compare {} with {}", l.type_name(), r.type_name()),
        )),
    }
}

/// Evaluate an expression that must be a `Bool`.
fn eval_bool(expr: &Expr, env: &mut Env) -> Result<bool, RuntimeError> {
    match eval_expr_in(expr, env)? {
        Value::Bool(b) => Ok(b),
        other => Err(RuntimeError::new(
            expr.span(),
            format!("expected a Bool, found {}", other.type_name()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a single expression and evaluate it.
    fn eval(src: &str) -> Result<Value, RuntimeError> {
        let expr = writ_parser::parse_expr(src).expect("source should parse");
        eval_expr(&expr)
    }

    /// Parse `stmts` as the body of a function and evaluate that block.
    fn eval_body(stmts: &str) -> Result<Value, RuntimeError> {
        let src = format!("fn main() {{ {stmts} }}");
        let result = writ_parser::parse(&src);
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        let writ_ast::Item::Function(f) = &result.module.items[0];
        eval_block(&f.body)
    }

    #[test]
    fn evaluates_literals() {
        assert_eq!(eval("42").unwrap(), Value::Int(42));
        assert_eq!(eval("true").unwrap(), Value::Bool(true));
        assert_eq!(eval(r#""hi""#).unwrap(), Value::Text("hi".into()));
    }

    #[test]
    fn precedence_matches_the_parsed_tree() {
        // Multiplication binds tighter than addition.
        assert_eq!(eval("1 + 2 * 3").unwrap(), Value::Int(7));
        // Parentheses override it.
        assert_eq!(eval("(1 + 2) * 3").unwrap(), Value::Int(9));
        // Left-associative subtraction.
        assert_eq!(eval("10 - 3 - 2").unwrap(), Value::Int(5));
    }

    #[test]
    fn comparison_and_logic() {
        assert_eq!(eval("1 + 2 < 4").unwrap(), Value::Bool(true));
        assert_eq!(eval("2 == 2").unwrap(), Value::Bool(true));
        assert_eq!(eval("2 != 3").unwrap(), Value::Bool(true));
        assert_eq!(eval("true && false").unwrap(), Value::Bool(false));
        assert_eq!(eval("false || true").unwrap(), Value::Bool(true));
        assert_eq!(eval("!(1 < 2)").unwrap(), Value::Bool(false));
    }

    #[test]
    fn logical_operators_short_circuit() {
        // Right side is a type error, but `&&` must not evaluate it when the
        // left is already false.
        assert_eq!(eval("false && (1 + 1)").unwrap(), Value::Bool(false));
        assert_eq!(eval("true || (1 + 1)").unwrap(), Value::Bool(true));
    }

    // --- Negative tests: bad programs are runtime errors, never panics.

    #[test]
    fn division_by_zero_is_a_runtime_error() {
        let e = eval("1 / 0").unwrap_err();
        assert!(e.message.contains("division by zero"), "{}", e.message);
    }

    #[test]
    fn overflow_is_a_runtime_error() {
        let e = eval("9223372036854775807 + 1").unwrap_err();
        assert!(e.message.contains("overflow"), "{}", e.message);
    }

    #[test]
    fn type_mismatch_is_a_runtime_error() {
        let e = eval(r#"1 + "x""#).unwrap_err();
        assert!(e.message.contains("Int"), "{}", e.message);
    }

    // --- let bindings (#12) ------------------------------------------------

    #[test]
    fn let_binding_resolves_in_later_expressions() {
        let v = eval_body("let x = 2; let y = x + 3; return y;").unwrap();
        assert_eq!(v, Value::Int(5));
    }

    #[test]
    fn return_stops_the_block() {
        let v = eval_body("let x = 1; return x; let y = 99;").unwrap();
        assert_eq!(v, Value::Int(1));
    }

    #[test]
    fn immutable_rebinding_is_a_checked_error() {
        let e = eval_body("let x = 1; let x = 2; return x;").unwrap_err();
        assert!(e.message.contains("immutable"), "{}", e.message);
    }

    #[test]
    fn mutable_rebinding_is_allowed() {
        let v = eval_body("let mut x = 1; let x = 2; return x;").unwrap();
        assert_eq!(v, Value::Int(2));
    }

    #[test]
    fn unbound_variable_is_a_runtime_error() {
        let e = eval_body("return missing;").unwrap_err();
        assert!(e.message.contains("unbound"), "{}", e.message);
    }
}

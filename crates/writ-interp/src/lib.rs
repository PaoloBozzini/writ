//! `writ-interp` — a tree-walking evaluator over the AST.
//!
//! A back end, not the source of truth: it evaluates the AST that `writ-parser`
//! produces and never performs static analysis. It also never panics on a bad
//! program — every arithmetic overflow, division by zero, or type mismatch
//! becomes a [`RuntimeError`] anchored to a span.
//!
//! This first stage evaluates the core expression forms: literals and the
//! arithmetic, comparison, and logical operators. Because it walks the tree the
//! parser built, operator precedence is already baked into the shape, so
//! `1 + 2 * 3` evaluates to `7`. Variables and calls arrive in later work.

mod value;

use writ_ast::{BinaryOp, Expr, Literal, LiteralKind, UnaryOp};

pub use value::{RuntimeError, Value};

/// Evaluate a core expression to a [`Value`].
///
/// # Errors
/// Returns a [`RuntimeError`] on overflow, division by zero, a type mismatch, or
/// an expression form not yet supported (variables and calls).
pub fn eval_expr(expr: &Expr) -> Result<Value, RuntimeError> {
    match expr {
        Expr::Literal(lit) => Ok(eval_literal(lit)),
        Expr::Unary { op, operand, span } => {
            let v = eval_expr(operand)?;
            eval_unary(*op, v, *span)
        }
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => eval_binary(*op, left, right, *span),
        Expr::Identifier { span, .. } => Err(RuntimeError::new(
            *span,
            "variables are not supported by the evaluator yet",
        )),
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
) -> Result<Value, RuntimeError> {
    // Logical operators short-circuit, so evaluate the right side lazily.
    match op {
        BinaryOp::And => {
            return match eval_bool(left)? {
                false => Ok(Value::Bool(false)),
                true => Ok(Value::Bool(eval_bool(right)?)),
            };
        }
        BinaryOp::Or => {
            return match eval_bool(left)? {
                true => Ok(Value::Bool(true)),
                false => Ok(Value::Bool(eval_bool(right)?)),
            };
        }
        _ => {}
    }

    let l = eval_expr(left)?;
    let r = eval_expr(right)?;
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
fn eval_bool(expr: &Expr) -> Result<bool, RuntimeError> {
    match eval_expr(expr)? {
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
}

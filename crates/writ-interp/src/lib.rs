//! `writ-interp` — a tree-walking evaluator over the AST.
//!
//! A back end, not the source of truth: it evaluates the AST that `writ-parser`
//! produces and never performs static analysis. It also never panics on a bad
//! program — every arithmetic overflow, division by zero, type mismatch, or bad
//! call becomes a [`RuntimeError`] anchored to a span.
//!
//! It evaluates the core expression forms, `let` bindings with lexical scope,
//! blocks with `return`, `if`/`else`, and user-defined function calls (nested
//! and recursive). Function calls are the sole channel through which values —
//! and, later, capabilities — enter a callee's scope: an [`Interpreter`] holds
//! an immutable table of the module's functions and binds arguments into a fresh
//! call frame.

mod env;
mod value;

use std::collections::HashMap;

use writ_ast::{
    BinaryOp, Block, Expr, Function, Literal, LiteralKind, Module, Span, Stmt, UnaryOp,
};

pub use env::Env;
pub use value::{RuntimeError, Value};

/// How evaluation of a statement flows: continue with a value, or return out of
/// the enclosing block carrying a value.
enum Control {
    Normal(Value),
    Return(Value),
}

/// Evaluate a single core expression in an empty environment (no functions in
/// scope).
///
/// # Errors
/// Returns a [`RuntimeError`] on overflow, division by zero, a type mismatch, an
/// unbound variable, or a call to an unknown function.
pub fn eval_expr(expr: &Expr) -> Result<Value, RuntimeError> {
    Interpreter::empty().eval_expr_in(expr, &mut Env::new())
}

/// Evaluate a block of statements in a fresh environment with no functions in
/// scope, returning the block's value (the value of a `return`, else `Unit`).
///
/// # Errors
/// Returns a [`RuntimeError`] on any evaluation failure inside the block.
pub fn eval_block(block: &Block) -> Result<Value, RuntimeError> {
    Interpreter::empty().eval_block_in(block, &mut Env::new())
}

/// Build an interpreter for `module` and call its `entry` function with `args`.
///
/// # Errors
/// Returns a [`RuntimeError`] if the module has duplicate function names, the
/// entry function is unknown, or evaluation fails.
pub fn run(module: &Module, entry: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
    Interpreter::new(module)?.call(entry, args)
}

/// A tree-walking interpreter over a module's functions.
///
/// The function table is an immutable `name -> &Function` map — a read-only
/// registry, not global mutable state — so evaluation stays locally reasoned:
/// authority and values flow only through explicit call arguments.
pub struct Interpreter<'m> {
    funcs: HashMap<&'m str, &'m Function>,
}

impl<'m> Interpreter<'m> {
    /// Build an interpreter from a module, indexing its functions by name.
    ///
    /// # Errors
    /// Returns a [`RuntimeError`] if two functions share a name — an ambiguous
    /// program the interpreter refuses rather than silently resolving.
    pub fn new(module: &'m Module) -> Result<Self, RuntimeError> {
        let mut funcs = HashMap::new();
        for item in &module.items {
            let writ_ast::Item::Function(f) = item;
            if funcs.insert(f.signature.name.as_str(), f).is_some() {
                return Err(RuntimeError::new(
                    f.signature.span,
                    format!("function `{}` is defined more than once", f.signature.name),
                ));
            }
        }
        Ok(Self { funcs })
    }

    /// An interpreter with no functions in scope.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            funcs: HashMap::new(),
        }
    }

    /// Call a function by name with already-evaluated arguments.
    ///
    /// # Errors
    /// Returns a [`RuntimeError`] if the function is unknown or evaluation fails.
    pub fn call(&self, name: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
        // No span is available from the public entry point; use an empty one.
        self.call_by_name(name, args, Span::new(0, 0))
    }

    fn call_by_name(
        &self,
        name: &str,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        let func = self
            .funcs
            .get(name)
            .copied()
            .ok_or_else(|| RuntimeError::new(span, format!("unknown function `{name}`")))?;
        self.call_function(func, args, span)
    }

    fn call_function(
        &self,
        func: &Function,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        let params = &func.signature.params;
        if args.len() != params.len() {
            return Err(RuntimeError::new(
                span,
                format!(
                    "function `{}` expects {} argument(s), got {}",
                    func.signature.name,
                    params.len(),
                    args.len()
                ),
            ));
        }
        // A fresh call frame: arguments are the only things in scope besides the
        // module's functions.
        let mut env = Env::new();
        for (param, value) in params.iter().zip(args) {
            env.define(&param.name, value, false)
                .map_err(|m| RuntimeError::new(param.span, m))?;
        }
        self.eval_block_in(&func.body, &mut env)
    }

    /// Evaluate a block, opening a nested lexical scope so its bindings do not
    /// leak to the caller. A `return` becomes the block's value.
    fn eval_block_in(&self, block: &Block, env: &mut Env) -> Result<Value, RuntimeError> {
        match self.run_block_stmts(block, env)? {
            Control::Normal(v) | Control::Return(v) => Ok(v),
        }
    }

    /// Run a block's statements in a nested scope, propagating a `return` outward
    /// as control flow rather than swallowing it into the block's value.
    fn run_block_stmts(&self, block: &Block, env: &mut Env) -> Result<Control, RuntimeError> {
        env.push_scope();
        let result = self.run_stmts(&block.stmts, env);
        env.pop_scope();
        result
    }

    /// Run a sequence of statements, stopping early on a `return`.
    fn run_stmts(&self, stmts: &[Stmt], env: &mut Env) -> Result<Control, RuntimeError> {
        let mut last = Value::Unit;
        for stmt in stmts {
            match self.eval_stmt(stmt, env)? {
                Control::Normal(v) => last = v,
                ret @ Control::Return(_) => return Ok(ret),
            }
        }
        Ok(Control::Normal(last))
    }

    fn eval_stmt(&self, stmt: &Stmt, env: &mut Env) -> Result<Control, RuntimeError> {
        match stmt {
            Stmt::Let {
                name,
                mutable,
                value,
                span,
                ..
            } => {
                let v = self.eval_expr_in(value, env)?;
                env.define(name, v, *mutable)
                    .map_err(|msg| RuntimeError::new(*span, msg))?;
                Ok(Control::Normal(Value::Unit))
            }
            Stmt::Expr(expr) => Ok(Control::Normal(self.eval_expr_in(expr, env)?)),
            Stmt::Return { value, .. } => {
                let v = match value {
                    Some(expr) => self.eval_expr_in(expr, env)?,
                    None => Value::Unit,
                };
                Ok(Control::Return(v))
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                if self.eval_bool(cond, env)? {
                    self.run_block_stmts(then_block, env)
                } else if let Some(else_block) = else_block {
                    self.run_block_stmts(else_block, env)
                } else {
                    Ok(Control::Normal(Value::Unit))
                }
            }
        }
    }

    /// Evaluate an expression in the given environment.
    fn eval_expr_in(&self, expr: &Expr, env: &mut Env) -> Result<Value, RuntimeError> {
        match expr {
            Expr::Literal(lit) => Ok(eval_literal(lit)),
            Expr::Unary { op, operand, span } => {
                let v = self.eval_expr_in(operand, env)?;
                eval_unary(*op, v, *span)
            }
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => self.eval_binary(*op, left, right, *span, env),
            Expr::Identifier { name, span } => env
                .lookup(name)
                .ok_or_else(|| RuntimeError::new(*span, format!("unbound variable `{name}`"))),
            Expr::Call { callee, args, span } => {
                let Expr::Identifier { name, .. } = callee.as_ref() else {
                    return Err(RuntimeError::new(
                        *span,
                        "only named functions can be called",
                    ));
                };
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval_expr_in(arg, env)?);
                }
                self.call_by_name(name, values, *span)
            }
        }
    }

    fn eval_binary(
        &self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        span: Span,
        env: &mut Env,
    ) -> Result<Value, RuntimeError> {
        // Logical operators short-circuit, so evaluate the right side lazily.
        match op {
            BinaryOp::And => {
                return match self.eval_bool(left, env)? {
                    false => Ok(Value::Bool(false)),
                    true => Ok(Value::Bool(self.eval_bool(right, env)?)),
                };
            }
            BinaryOp::Or => {
                return match self.eval_bool(left, env)? {
                    true => Ok(Value::Bool(true)),
                    false => Ok(Value::Bool(self.eval_bool(right, env)?)),
                };
            }
            _ => {}
        }

        let l = self.eval_expr_in(left, env)?;
        let r = self.eval_expr_in(right, env)?;
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

    /// Evaluate an expression that must be a `Bool`.
    fn eval_bool(&self, expr: &Expr, env: &mut Env) -> Result<bool, RuntimeError> {
        match self.eval_expr_in(expr, env)? {
            Value::Bool(b) => Ok(b),
            other => Err(RuntimeError::new(
                expr.span(),
                format!("expected a Bool, found {}", other.type_name()),
            )),
        }
    }
}

fn eval_literal(lit: &Literal) -> Value {
    match &lit.kind {
        LiteralKind::Int(n) => Value::Int(*n),
        LiteralKind::Bool(b) => Value::Bool(*b),
        LiteralKind::Text(s) => Value::Text(s.clone()),
    }
}

fn eval_unary(op: UnaryOp, v: Value, span: Span) -> Result<Value, RuntimeError> {
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

fn arithmetic(op: BinaryOp, l: Value, r: Value, span: Span) -> Result<Value, RuntimeError> {
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

fn compare(op: BinaryOp, l: Value, r: Value, span: Span) -> Result<Value, RuntimeError> {
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
fn equal(l: &Value, r: &Value, span: Span) -> Result<bool, RuntimeError> {
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

    /// Parse a whole program and call `entry` with `args`.
    fn run_program(src: &str, entry: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
        let result = writ_parser::parse(src);
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        run(&result.module, entry, args)
    }

    #[test]
    fn evaluates_literals() {
        assert_eq!(eval("42").unwrap(), Value::Int(42));
        assert_eq!(eval("true").unwrap(), Value::Bool(true));
        assert_eq!(eval(r#""hi""#).unwrap(), Value::Text("hi".into()));
    }

    #[test]
    fn precedence_matches_the_parsed_tree() {
        assert_eq!(eval("1 + 2 * 3").unwrap(), Value::Int(7));
        assert_eq!(eval("(1 + 2) * 3").unwrap(), Value::Int(9));
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

    // --- if / else (#53) ---------------------------------------------------

    #[test]
    fn if_runs_the_matching_branch() {
        assert_eq!(
            eval_body("if 5 > 3 { return 1; } return 2;").unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            eval_body("if 1 > 3 { return 1; } return 2;").unwrap(),
            Value::Int(2)
        );
    }

    #[test]
    fn if_else_selects_the_else_branch() {
        assert_eq!(
            eval_body("if false { return 1; } else { return 2; }").unwrap(),
            Value::Int(2)
        );
    }

    #[test]
    fn else_if_chains() {
        let body = "let x = 2;\
            if x == 1 { return 10; } else if x == 2 { return 20; } else { return 30; }\
            return 0;";
        assert_eq!(eval_body(body).unwrap(), Value::Int(20));
    }

    #[test]
    fn non_bool_condition_is_a_runtime_error() {
        let e = eval_body("if 1 { return 1; } return 0;").unwrap_err();
        assert!(e.message.contains("Bool"), "{}", e.message);
    }

    // --- functions and calls (#13) -----------------------------------------

    #[test]
    fn function_with_parameters_and_return() {
        let src = "fn add(a: Int, b: Int) -> Int { return a + b; }";
        assert_eq!(
            run_program(src, "add", vec![Value::Int(3), Value::Int(4)]).unwrap(),
            Value::Int(7)
        );
    }

    #[test]
    fn nested_calls_propagate_return_values() {
        let src = "\
fn inc(x: Int) -> Int { return x + 1; }
fn double(x: Int) -> Int { return x + x; }
fn main() -> Int { return double(inc(4)); }
";
        assert_eq!(run_program(src, "main", vec![]).unwrap(), Value::Int(10));
    }

    #[test]
    fn recursion_terminates_via_a_base_case() {
        let src = "\
fn fact(n: Int) -> Int {
    if n <= 1 { return 1; }
    return n * fact(n - 1);
}
";
        assert_eq!(
            run_program(src, "fact", vec![Value::Int(5)]).unwrap(),
            Value::Int(120)
        );
    }

    #[test]
    fn wrong_argument_count_is_a_runtime_error() {
        let src = "fn add(a: Int, b: Int) -> Int { return a + b; }";
        let e = run_program(src, "add", vec![Value::Int(1)]).unwrap_err();
        assert!(e.message.contains("expects 2"), "{}", e.message);
    }

    #[test]
    fn unknown_function_is_a_runtime_error() {
        let src = "fn main() -> Int { return nope(1); }";
        let e = run_program(src, "main", vec![]).unwrap_err();
        assert!(e.message.contains("unknown function"), "{}", e.message);
    }

    #[test]
    fn duplicate_function_names_are_refused() {
        let src = "fn f() -> Int { return 1; } fn f() -> Int { return 2; }";
        let result = writ_parser::parse(src);
        let e = run(&result.module, "f", vec![]).unwrap_err();
        assert!(e.message.contains("more than once"), "{}", e.message);
    }
}

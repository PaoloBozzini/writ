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

use std::cell::RefCell;
use std::collections::HashMap;

use writ_ast::{
    BinaryOp, Block, Expr, Function, Literal, LiteralKind, Module, Span, Stmt, UnaryOp,
};

/// The name of the sole built-in for now. Kept tiny on purpose; the real stdlib
/// (which threads capabilities through effectful built-ins) comes later.
const PRINT: &str = "print";

/// The capability-narrowing built-in: `grant<A>(cap)`.
const GRANT: &str = "grant";

/// The taint-removing built-in (identity at runtime): `sanitize(x)`.
const SANITIZE: &str = "sanitize";

/// The type-head marking a capability parameter.
const CAP: &str = "Cap";

pub use env::Env;
pub use value::{Blame, RuntimeError, Value};

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
    // Desugar contracts into shared `Check` nodes before executing, so contract
    // semantics come from the one shared lowering, not a back-end special case.
    let lowered = writ_lower::lower(module);
    Interpreter::new(&lowered)?.call(entry, args)
}

/// Run a module's `main`, handing it the **root capability** for each of its
/// capability parameters — the sole source of authority, provided by the
/// runtime. Non-capability parameters (if any) receive `Unit`.
///
/// # Errors
/// Returns a [`RuntimeError`] if there is no `main`, or evaluation fails.
pub fn run_main(module: &Module) -> Result<Value, RuntimeError> {
    let lowered = writ_lower::lower(module);
    let interp = Interpreter::new(&lowered)?;
    let main = interp
        .funcs
        .get("main")
        .copied()
        .ok_or_else(|| RuntimeError::new(Span::new(0, 0), "no `main` function"))?;
    let args = main
        .signature
        .params
        .iter()
        .map(|p| {
            if p.ty.name == CAP {
                let authority =
                    p.ty.args
                        .first()
                        .map_or_else(|| "Root".to_string(), |a| a.name.clone());
                Value::Capability { authority }
            } else {
                Value::Unit
            }
        })
        .collect();
    interp.call("main", args)
}

/// A tree-walking interpreter over a module's functions.
///
/// The function table is an immutable `name -> &Function` map — a read-only
/// registry, not global mutable state — so evaluation stays locally reasoned:
/// authority and values flow only through explicit call arguments.
pub struct Interpreter<'m> {
    funcs: HashMap<&'m str, &'m Function>,
    /// Sum-type constructors declared by the module: variant name → arity. Used
    /// to evaluate constructor expressions and to classify bare-identifier
    /// patterns as nullary variants.
    constructors: HashMap<&'m str, usize>,
    /// Output collected from the `print` built-in, one entry per call. Held here
    /// rather than written to ambient stdout so evaluation is deterministic and
    /// testable — and so effects stay local to an interpreter instance.
    output: RefCell<Vec<String>>,
}

impl<'m> Interpreter<'m> {
    /// Build an interpreter from a module, indexing its functions by name.
    ///
    /// # Errors
    /// Returns a [`RuntimeError`] if two functions share a name — an ambiguous
    /// program the interpreter refuses rather than silently resolving.
    pub fn new(module: &'m Module) -> Result<Self, RuntimeError> {
        let mut funcs = HashMap::new();
        let mut constructors = HashMap::new();
        for item in &module.items {
            match item {
                writ_ast::Item::Function(f) => {
                    if funcs.insert(f.signature.name.as_str(), f).is_some() {
                        return Err(RuntimeError::new(
                            f.signature.span,
                            format!("function `{}` is defined more than once", f.signature.name),
                        ));
                    }
                }
                writ_ast::Item::Type(decl) => {
                    for variant in &decl.variants {
                        constructors.insert(variant.name.as_str(), variant.fields.len());
                    }
                }
            }
        }
        Ok(Self {
            funcs,
            constructors,
            output: RefCell::new(Vec::new()),
        })
    }

    /// An interpreter with no functions in scope.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            funcs: HashMap::new(),
            constructors: HashMap::new(),
            output: RefCell::new(Vec::new()),
        }
    }

    /// The lines emitted by `print` so far, in order.
    #[must_use]
    pub fn output(&self) -> Vec<String> {
        self.output.borrow().clone()
    }

    /// The text built-ins. `Text` is a sequence of Unicode scalar values, so
    /// `text_len`, `char_at`, and `substring` are all char-based; out-of-range
    /// access is a runtime error.
    fn builtin_text(
        &self,
        name: &str,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        let text = |v: &Value| -> Result<String, RuntimeError> {
            match v {
                Value::Text(s) => Ok(s.clone()),
                other => Err(RuntimeError::new(
                    span,
                    format!(
                        "`{name}` expects a `Text` argument, got {}",
                        other.type_name()
                    ),
                )),
            }
        };
        let int = |v: &Value| -> Result<i64, RuntimeError> {
            match v {
                Value::Int(n) => Ok(*n),
                other => Err(RuntimeError::new(
                    span,
                    format!(
                        "`{name}` expects an `Int` argument, got {}",
                        other.type_name()
                    ),
                )),
            }
        };
        let arity = |n: usize| -> Result<(), RuntimeError> {
            if args.len() == n {
                Ok(())
            } else {
                Err(RuntimeError::new(
                    span,
                    format!("`{name}` expects {n} argument(s), got {}", args.len()),
                ))
            }
        };
        match name {
            "concat" => {
                arity(2)?;
                Ok(Value::Text(text(&args[0])? + &text(&args[1])?))
            }
            "text_len" => {
                arity(1)?;
                Ok(Value::Int(text(&args[0])?.chars().count() as i64))
            }
            "char_at" => {
                arity(2)?;
                let s = text(&args[0])?;
                let i = int(&args[1])?;
                let c = usize::try_from(i)
                    .ok()
                    .and_then(|i| s.chars().nth(i))
                    .ok_or_else(|| {
                        RuntimeError::new(span, format!("`char_at` index {i} is out of range"))
                    })?;
                Ok(Value::Text(c.to_string()))
            }
            "substring" => {
                arity(3)?;
                let s = text(&args[0])?;
                let start = int(&args[1])?;
                let end = int(&args[2])?;
                let len = s.chars().count() as i64;
                if start < 0 || end < start || end > len {
                    return Err(RuntimeError::new(
                        span,
                        format!("`substring` range {start}..{end} is out of bounds (len {len})"),
                    ));
                }
                let sub: String = s
                    .chars()
                    .skip(start as usize)
                    .take((end - start) as usize)
                    .collect();
                Ok(Value::Text(sub))
            }
            _ => unreachable!("dispatched only for text built-ins"),
        }
    }

    /// The `print` built-in: emit one line per call. Requires exactly one
    /// argument.
    fn builtin_print(&self, args: Vec<Value>, span: Span) -> Result<Value, RuntimeError> {
        let [value] = <[Value; 1]>::try_from(args).map_err(|args| {
            RuntimeError::new(
                span,
                format!("`print` expects 1 argument, got {}", args.len()),
            )
        })?;
        self.output.borrow_mut().push(value.to_string());
        Ok(Value::Unit)
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
        if let Some(func) = self.funcs.get(name).copied() {
            return self.call_function(func, args, span);
        }
        // A user function of the same name would have shadowed it above.
        if name == PRINT {
            return self.builtin_print(args, span);
        }
        // `sanitize` is an identity at runtime — taint is a compile-time property
        // with no runtime representation.
        if name == SANITIZE {
            let [value] = <[Value; 1]>::try_from(args).map_err(|args| {
                RuntimeError::new(
                    span,
                    format!("`sanitize` expects 1 argument, got {}", args.len()),
                )
            })?;
            return Ok(value);
        }
        if matches!(name, "concat" | "text_len" | "char_at" | "substring") {
            return self.builtin_text(name, args, span);
        }
        Err(RuntimeError::new(
            span,
            format!("unknown function `{name}`"),
        ))
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

        // Contracts are **not** special-cased here. The `writ-lower` pass has
        // already desugared `requires` / `ensures` into `Stmt::Check` nodes in
        // the body (preconditions first, postconditions on every exit with
        // `result` bound), so a call just evaluates the body and the shared
        // `Check` semantics do the rest. This is the single place contract
        // checking lives, shared with the native back end.
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
            // A lowered contract check (see `writ-lower`). Evaluating the
            // predicate to `false` fails the call, blaming the side the lowering
            // recorded: a lowered precondition blames the caller, a lowered
            // postcondition blames the implementation.
            Stmt::Check {
                predicate,
                blame,
                span,
            } => {
                if self.eval_bool(predicate, env)? {
                    Ok(Control::Normal(Value::Unit))
                } else {
                    Err(match blame {
                        Blame::Caller => RuntimeError::precondition(*span),
                        Blame::Implementation => RuntimeError::postcondition(*span),
                    })
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
            Expr::Identifier { name, span } => {
                if let Some(value) = env.lookup(name) {
                    return Ok(value);
                }
                // A bare name that is a nullary constructor (e.g. `None`) builds
                // its variant value.
                if self.constructors.get(name.as_str()) == Some(&0) {
                    return Ok(Value::Variant {
                        name: name.clone(),
                        fields: Vec::new(),
                    });
                }
                Err(RuntimeError::new(
                    *span,
                    format!("unbound variable `{name}`"),
                ))
            }
            Expr::Call {
                callee,
                type_args,
                args,
                span,
            } => {
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
                // `grant<A>(cap)` narrows to a capability tagged with authority A.
                // Narrowing validity is checked statically; at runtime it just
                // mints the narrowed token. The source is evaluated (above) so a
                // bad source still errors, but is otherwise opaque here.
                if name == GRANT {
                    let authority = type_args
                        .first()
                        .map_or_else(|| "Root".to_string(), |t| t.name.clone());
                    return Ok(Value::Capability { authority });
                }
                // A call to a constructor builds a variant value; otherwise it is
                // an ordinary function or built-in call.
                if let Some(&arity) = self.constructors.get(name.as_str()) {
                    if values.len() != arity {
                        return Err(RuntimeError::new(
                            *span,
                            format!(
                                "constructor `{name}` expects {arity} argument(s), got {}",
                                values.len()
                            ),
                        ));
                    }
                    return Ok(Value::Variant {
                        name: name.clone(),
                        fields: values,
                    });
                }
                self.call_by_name(name, values, *span)
            }
            Expr::Match {
                scrutinee,
                arms,
                span,
            } => {
                let value = self.eval_expr_in(scrutinee, env)?;
                for arm in arms {
                    env.push_scope();
                    if self.try_match(&arm.pattern, &value, env) {
                        let result = self.eval_expr_in(&arm.body, env);
                        env.pop_scope();
                        return result;
                    }
                    env.pop_scope();
                }
                Err(RuntimeError::new(
                    *span,
                    format!("no match arm covers the value `{value}`"),
                ))
            }
            Expr::Member { span, .. } => Err(RuntimeError::new(
                *span,
                "module member access is not resolved yet",
            )),
        }
    }

    /// Try to match `pattern` against `value`, binding any captured names into
    /// the current scope. Returns whether it matched. A bare identifier is a
    /// nullary-variant test if it names a constructor, otherwise a binding.
    fn try_match(&self, pattern: &writ_ast::Pattern, value: &Value, env: &mut Env) -> bool {
        use writ_ast::Pattern;
        match pattern {
            Pattern::Wildcard { .. } => true,
            Pattern::Ident { name, .. } => {
                if self.constructors.get(name.as_str()) == Some(&0) {
                    matches!(value, Value::Variant { name: v, fields } if v == name && fields.is_empty())
                } else {
                    // A binding pattern always matches and captures the value.
                    let _ = env.define(name, value.clone(), false);
                    true
                }
            }
            Pattern::Variant { name, args, .. } => {
                let Value::Variant { name: v, fields } = value else {
                    return false;
                };
                if v != name || fields.len() != args.len() {
                    return false;
                }
                args.iter()
                    .zip(fields)
                    .all(|(sub, val)| self.try_match(sub, val, env))
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
        (
            Value::Variant {
                name: a,
                fields: fa,
            },
            Value::Variant {
                name: b,
                fields: fb,
            },
        ) => {
            if a != b || fa.len() != fb.len() {
                return Ok(false);
            }
            for (x, y) in fa.iter().zip(fb) {
                if !equal(x, y, span)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
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
        let writ_ast::Item::Function(f) = &result.module.items[0] else {
            panic!("expected a function")
        };
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

    // --- stdlib: print (#15) -----------------------------------------------

    #[test]
    fn print_builtin_collects_output() {
        let src = "\
fn main() {
    print(\"a\");
    print(1 + 2);
    print(true);
}
";
        let result = writ_parser::parse(src);
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        let interp = Interpreter::new(&result.module).unwrap();
        interp.call("main", vec![]).unwrap();
        assert_eq!(
            interp.output(),
            vec!["a".to_string(), "3".to_string(), "true".to_string()]
        );
    }

    // --- sum types: runtime match (#14) ------------------------------------

    #[test]
    fn match_dispatches_on_the_variant_and_binds_payload() {
        let src = "\
type Option = Some(Int) | None
fn unwrap_or(o: Option, fallback: Int) -> Int {
    return match o {
        Some(x) => x,
        None    => fallback,
    };
}
fn some_case() -> Int { return unwrap_or(Some(7), 0); }
fn none_case() -> Int { return unwrap_or(None, 42); }
";
        assert_eq!(
            run_program(src, "some_case", vec![]).unwrap(),
            Value::Int(7)
        );
        assert_eq!(
            run_program(src, "none_case", vec![]).unwrap(),
            Value::Int(42)
        );
    }

    #[test]
    fn wildcard_and_binding_patterns_match() {
        let src = "\
type Color = Red | Green | Blue
fn code(c: Color) -> Int {
    return match c {
        Red => 1,
        other => 0,
    };
}
fn red() -> Int { return code(Red); }
fn blue() -> Int { return code(Blue); }
";
        assert_eq!(run_program(src, "red", vec![]).unwrap(), Value::Int(1));
        // Green/Blue fall through to the `other` binding arm.
        assert_eq!(run_program(src, "blue", vec![]).unwrap(), Value::Int(0));
    }

    #[test]
    fn constructor_arity_mismatch_is_a_runtime_error() {
        let src = "\
type Pair = Pair(Int, Int)
fn f() -> Int { return match Pair(1) { Pair(a, b) => a, _ => 0 }; }
";
        let e = run_program(src, "f", vec![]).unwrap_err();
        assert!(e.message.contains("expects 2"), "{}", e.message);
    }

    #[test]
    fn a_value_matching_no_arm_is_a_runtime_error() {
        // No wildcard, and Blue is not covered — detected at evaluation (static
        // exhaustiveness is #17).
        let src = "\
type Color = Red | Green | Blue
fn code(c: Color) -> Int { return match c { Red => 1, Green => 2 }; }
fn f() -> Int { return code(Blue); }
";
        let e = run_program(src, "f", vec![]).unwrap_err();
        assert!(e.message.contains("no match arm"), "{}", e.message);
    }

    // --- capabilities: root + grant at runtime (#22) -----------------------

    #[test]
    fn run_main_hands_main_a_root_capability_that_grant_narrows() {
        let src = "\
fn write_line(out: Cap<Write>, msg: Text) uses { Write } { return; }
fn main(root: Cap<Root>) uses { Write } {
    write_line(grant<Write>(root), \"ok\");
}
";
        let result = writ_parser::parse(src);
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        // The runtime supplies the root capability; narrowing and the effectful
        // call run without error.
        assert_eq!(run_main(&result.module).unwrap(), Value::Unit);
    }

    #[test]
    fn grant_mints_a_capability_tagged_with_its_authority() {
        let src = "fn main(root: Cap<Root>) -> Cap<Write> { return grant<Write>(root); }";
        let result = writ_parser::parse(src);
        // (The static checker would reject returning a capability; here we only
        // exercise the runtime value that `grant` produces.)
        let v = run_main(&result.module).unwrap();
        assert_eq!(
            v,
            Value::Capability {
                authority: "Write".to_string()
            }
        );
    }

    // --- contracts: runtime checking with blame (#26) ----------------------

    #[test]
    fn precondition_holds_then_body_runs() {
        let src = "fn half(n: Int) -> Int requires n > 0 { return n / 2; }";
        assert_eq!(
            run_program(src, "half", vec![Value::Int(4)]).unwrap(),
            Value::Int(2)
        );
    }

    #[test]
    fn failed_precondition_blames_the_caller() {
        let src = "fn half(n: Int) -> Int requires n > 0 { return n / 2; }";
        let e = run_program(src, "half", vec![Value::Int(0)]).unwrap_err();
        assert_eq!(e.blame, Some(Blame::Caller));
        assert!(e.message.contains("precondition"), "{}", e.message);
    }

    #[test]
    fn satisfied_postcondition_passes() {
        // `result` refers to the return value; the postcondition also sees params.
        let src = "fn inc(n: Int) -> Int ensures result > n { return n + 1; }";
        assert_eq!(
            run_program(src, "inc", vec![Value::Int(5)]).unwrap(),
            Value::Int(6)
        );
    }

    #[test]
    fn failed_postcondition_blames_the_implementation() {
        // A wrong-but-harmless absolute value: returns its input unchanged.
        let src = "fn abs(n: Int) -> Int ensures result >= 0 { return n; }";
        // Allowed input, wrong answer => implementation is blamed.
        let e = run_program(src, "abs", vec![Value::Int(0 - 5)]).unwrap_err();
        assert_eq!(e.blame, Some(Blame::Implementation));
        assert!(e.message.contains("postcondition"), "{}", e.message);
        // A correct result satisfies it.
        assert_eq!(
            run_program(src, "abs", vec![Value::Int(5)]).unwrap(),
            Value::Int(5)
        );
    }

    #[test]
    fn ordinary_runtime_errors_carry_no_blame() {
        let src = "fn boom(n: Int) -> Int { return n / 0; }";
        let e = run_program(src, "boom", vec![Value::Int(1)]).unwrap_err();
        assert_eq!(e.blame, None);
    }

    #[test]
    fn print_with_wrong_arity_is_a_runtime_error() {
        let src = "fn main() { print(1, 2); }";
        let result = writ_parser::parse(src);
        let interp = Interpreter::new(&result.module).unwrap();
        let e = interp.call("main", vec![]).unwrap_err();
        assert!(e.message.contains("`print` expects 1"), "{}", e.message);
    }
}

//! The native back end: emit **C** from a checked, lowered Writ module.
//!
//! This is a back end, not the source of truth — the tree-walking interpreter
//! (M2) remains the semantic reference, and generated programs must agree with
//! it (see the differential corpus). The emit target is **C** (decision on
//! #29): the leanest path to a standalone binary via the system C compiler.
//!
//! Codegen consumes the **lowered** AST (see `writ-lower`), so contracts already
//! appear as [`Stmt::Check`] nodes — this back end implements only that one
//! shared construct, never `requires` / `ensures` directly. That is exactly the
//! anti-drift property #38 buys: the interpreter and this back end share a
//! single contract semantics.
//!
//! ## Scope
//! This first increment covers the **core** language: `Int` / `Bool`, the
//! arithmetic / comparison / boolean operators (with the interpreter's checked
//! overflow and division-by-zero semantics, and short-circuit `&&` / `||`),
//! `let` / `if` / `return`, function calls, `print`, and lowered contract
//! checks. Constructs not yet supported (`Text`, sum types, `match`, capability
//! narrowing) are reported as a [`CodegenError`] rather than mis-compiled;
//! they land in a follow-up.

use std::fmt::Write as _;

use writ_ast::{BinaryOp, Blame, Block, Expr, Item, LiteralKind, Module, Span, Stmt, UnaryOp};

/// A construct the back end cannot yet emit, anchored to its source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenError {
    pub message: String,
    pub span: Span,
}

impl CodegenError {
    fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }
}

/// The C runtime prelude: the tagged `WValue` and the operator helpers. Every
/// helper reproduces the interpreter's semantics exactly — checked arithmetic,
/// division-by-zero traps, and the interpreter's value formatting for `print`.
const PRELUDE: &str = r#"#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef enum { W_INT, W_BOOL, W_UNIT } WTag;
typedef struct { WTag tag; int64_t i; } WValue;

static WValue w_int(int64_t x) { WValue v; v.tag = W_INT; v.i = x; return v; }
static WValue w_bool(int b) { WValue v; v.tag = W_BOOL; v.i = b ? 1 : 0; return v; }
static WValue w_unit(void) { WValue v; v.tag = W_UNIT; v.i = 0; return v; }
static int w_as_bool(WValue v) { return v.i != 0; }

static void w_trap(const char *msg) { fprintf(stderr, "%s\n", msg); exit(1); }

static WValue w_add(WValue a, WValue b) { int64_t r; if (__builtin_add_overflow(a.i, b.i, &r)) w_trap("integer overflow"); return w_int(r); }
static WValue w_sub(WValue a, WValue b) { int64_t r; if (__builtin_sub_overflow(a.i, b.i, &r)) w_trap("integer overflow"); return w_int(r); }
static WValue w_mul(WValue a, WValue b) { int64_t r; if (__builtin_mul_overflow(a.i, b.i, &r)) w_trap("integer overflow"); return w_int(r); }
static WValue w_div(WValue a, WValue b) { if (b.i == 0) w_trap("division by zero"); if (a.i == INT64_MIN && b.i == -1) w_trap("integer overflow"); return w_int(a.i / b.i); }
static WValue w_rem(WValue a, WValue b) { if (b.i == 0) w_trap("division by zero"); if (a.i == INT64_MIN && b.i == -1) w_trap("integer overflow"); return w_int(a.i % b.i); }
static WValue w_neg(WValue a) { int64_t r; if (__builtin_sub_overflow((int64_t)0, a.i, &r)) w_trap("integer overflow negating value"); return w_int(r); }
static WValue w_not(WValue a) { return w_bool(a.i == 0); }
static WValue w_lt(WValue a, WValue b) { return w_bool(a.i < b.i); }
static WValue w_le(WValue a, WValue b) { return w_bool(a.i <= b.i); }
static WValue w_gt(WValue a, WValue b) { return w_bool(a.i > b.i); }
static WValue w_ge(WValue a, WValue b) { return w_bool(a.i >= b.i); }
static WValue w_eq(WValue a, WValue b) { return w_bool(a.tag == b.tag && a.i == b.i); }
static WValue w_ne(WValue a, WValue b) { return w_bool(!(a.tag == b.tag && a.i == b.i)); }
static WValue w_print(WValue v) {
    switch (v.tag) {
        case W_INT: printf("%lld\n", (long long) v.i); break;
        case W_BOOL: printf("%s\n", v.i ? "true" : "false"); break;
        case W_UNIT: printf("()\n"); break;
    }
    return w_unit();
}
"#;

/// Emit a complete C translation unit for `module`.
///
/// The module must be **checked** and **lowered** (contracts already desugared
/// into `Stmt::Check`) and **linked** into a single module (no imports).
///
/// # Errors
/// Returns a [`CodegenError`] if the module uses a construct this back end does
/// not yet support, or has no `main` function.
pub fn emit_c(module: &Module) -> Result<String, CodegenError> {
    let funcs: Vec<_> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Function(f) => Some(f),
            Item::Type(_) => None,
        })
        .collect();

    let main = funcs
        .iter()
        .find(|f| f.signature.name == "main")
        .ok_or_else(|| CodegenError::new(Span::new(0, 0), "no `main` function to build"))?;

    let mut out = String::new();
    out.push_str(PRELUDE);
    out.push('\n');

    // Forward declarations, so call order does not matter.
    for f in &funcs {
        let _ = writeln!(
            out,
            "static WValue {};",
            proto(&f.signature.name, f.signature.params.len())
        );
    }
    out.push('\n');

    // Definitions.
    for f in &funcs {
        let _ = writeln!(
            out,
            "static WValue {} {{",
            proto(&f.signature.name, f.signature.params.len())
        );
        // Bind parameters to their local C names.
        for (i, p) in f.signature.params.iter().enumerate() {
            let _ = writeln!(out, "    WValue {} = p{};", local(&p.name), i);
        }
        emit_block(&f.body, 1, &mut out)?;
        // A Unit-returning path that falls off the end still yields a value.
        out.push_str("    return w_unit();\n");
        out.push_str("}\n\n");
    }

    // The C entry point calls Writ's `main`, handing each parameter a unit
    // placeholder — capability parameters carry no runtime data (there are no
    // effectful built-ins), so `main` never inspects them.
    out.push_str("int main(void) {\n");
    let args = vec!["w_unit()"; main.signature.params.len()].join(", ");
    let _ = writeln!(out, "    {}({});", cname(&main.signature.name), args);
    out.push_str("    return 0;\n}\n");

    Ok(out)
}

/// A function prototype's `name(WValue p0, ...)` fragment (without return type).
fn proto(name: &str, arity: usize) -> String {
    let params: Vec<String> = (0..arity).map(|i| format!("WValue p{i}")).collect();
    format!("{}({})", cname(name), params.join(", "))
}

/// A collision-free C identifier for a Writ function (may be qualified, e.g.
/// `math.add`). The `wf_` prefix also keeps Writ's `main` from clashing with C's.
fn cname(name: &str) -> String {
    format!("wf_{}", sanitize(name))
}

/// A C identifier for a local binding or parameter.
fn local(name: &str) -> String {
    format!("v_{}", sanitize(name))
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn indent(level: usize, out: &mut String) {
    for _ in 0..level {
        out.push_str("    ");
    }
}

fn emit_block(block: &Block, level: usize, out: &mut String) -> Result<(), CodegenError> {
    for stmt in &block.stmts {
        emit_stmt(stmt, level, out)?;
    }
    Ok(())
}

fn emit_stmt(stmt: &Stmt, level: usize, out: &mut String) -> Result<(), CodegenError> {
    match stmt {
        Stmt::Let { name, value, .. } => {
            indent(level, out);
            let _ = writeln!(out, "WValue {} = {};", local(name), emit_expr(value)?);
        }
        Stmt::Expr(e) => {
            indent(level, out);
            let _ = writeln!(out, "{};", emit_expr(e)?);
        }
        Stmt::Return { value, .. } => {
            indent(level, out);
            match value {
                Some(e) => {
                    let _ = writeln!(out, "return {};", emit_expr(e)?);
                }
                None => out.push_str("return w_unit();\n"),
            }
        }
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            indent(level, out);
            let _ = writeln!(out, "if (w_as_bool({})) {{", emit_expr(cond)?);
            emit_block(then_block, level + 1, out)?;
            indent(level, out);
            out.push('}');
            if let Some(else_block) = else_block {
                out.push_str(" else {\n");
                emit_block(else_block, level + 1, out)?;
                indent(level, out);
                out.push('}');
            }
            out.push('\n');
        }
        // A lowered contract check. Trapping reproduces the interpreter's exact
        // message, so runtime failures read identically across back ends.
        Stmt::Check {
            predicate, blame, ..
        } => {
            let msg = match blame {
                Blame::Caller => "precondition violated (blame: caller)",
                Blame::Implementation => "postcondition violated (blame: implementation)",
            };
            indent(level, out);
            let _ = writeln!(
                out,
                "if (!w_as_bool({})) w_trap(\"{}\");",
                emit_expr(predicate)?,
                msg
            );
        }
    }
    Ok(())
}

fn emit_expr(expr: &Expr) -> Result<String, CodegenError> {
    match expr {
        Expr::Literal(lit) => match &lit.kind {
            LiteralKind::Int(n) => Ok(format!("w_int({n})")),
            LiteralKind::Bool(b) => Ok(format!("w_bool({})", i32::from(*b))),
            LiteralKind::Text(_) => Err(CodegenError::new(
                lit.span,
                "text literals are not supported by the C back end yet",
            )),
        },
        Expr::Identifier { name, span } => {
            // Constructors (nullary variants) are sum-type values — not yet
            // supported. A plain variable maps to its local C name.
            if name.chars().next().is_some_and(char::is_uppercase) {
                return Err(CodegenError::new(
                    *span,
                    format!("sum-type constructor `{name}` is not supported by the C back end yet"),
                ));
            }
            Ok(local(name))
        }
        Expr::Unary { op, operand, .. } => {
            let inner = emit_expr(operand)?;
            Ok(match op {
                UnaryOp::Neg => format!("w_neg({inner})"),
                UnaryOp::Not => format!("w_not({inner})"),
            })
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            let l = emit_expr(left)?;
            let r = emit_expr(right)?;
            Ok(match op {
                // Short-circuit operators emit C `&&` / `||` so a side-effecting
                // right operand is skipped exactly as the interpreter skips it.
                BinaryOp::And => format!("w_bool(w_as_bool({l}) && w_as_bool({r}))"),
                BinaryOp::Or => format!("w_bool(w_as_bool({l}) || w_as_bool({r}))"),
                BinaryOp::Add => format!("w_add({l}, {r})"),
                BinaryOp::Sub => format!("w_sub({l}, {r})"),
                BinaryOp::Mul => format!("w_mul({l}, {r})"),
                BinaryOp::Div => format!("w_div({l}, {r})"),
                BinaryOp::Rem => format!("w_rem({l}, {r})"),
                BinaryOp::Lt => format!("w_lt({l}, {r})"),
                BinaryOp::Le => format!("w_le({l}, {r})"),
                BinaryOp::Gt => format!("w_gt({l}, {r})"),
                BinaryOp::Ge => format!("w_ge({l}, {r})"),
                BinaryOp::Eq => format!("w_eq({l}, {r})"),
                BinaryOp::Ne => format!("w_ne({l}, {r})"),
            })
        }
        Expr::Call {
            callee, args, span, ..
        } => emit_call(callee, args, *span),
        Expr::Match { span, .. } => Err(CodegenError::new(
            *span,
            "`match` is not supported by the C back end yet",
        )),
        Expr::Member { span, .. } => Err(CodegenError::new(
            *span,
            "member access is not supported by the C back end yet",
        )),
    }
}

fn emit_call(callee: &Expr, args: &[Expr], span: Span) -> Result<String, CodegenError> {
    let Expr::Identifier { name, .. } = callee else {
        return Err(CodegenError::new(
            span,
            "only named functions can be called in the C back end yet",
        ));
    };
    let emitted: Result<Vec<String>, _> = args.iter().map(emit_expr).collect();
    let emitted = emitted?;

    // Built-ins. `print` writes a line; `sanitize` is a runtime identity (taint
    // is a compile-time property with no runtime representation).
    if name == "print" {
        let arg = emitted
            .first()
            .ok_or_else(|| CodegenError::new(span, "`print` expects 1 argument"))?;
        return Ok(format!("w_print({arg})"));
    }
    if name == "sanitize" {
        let arg = emitted
            .first()
            .ok_or_else(|| CodegenError::new(span, "`sanitize` expects 1 argument"))?;
        return Ok(format!("({arg})"));
    }
    if name == "grant" {
        return Err(CodegenError::new(
            span,
            "capability narrowing (`grant`) is not supported by the C back end yet",
        ));
    }
    // A constructor call starts with an uppercase name — a sum-type value.
    if name.chars().next().is_some_and(char::is_uppercase) {
        return Err(CodegenError::new(
            span,
            format!("sum-type constructor `{name}` is not supported by the C back end yet"),
        ));
    }
    Ok(format!("{}({})", cname(name), emitted.join(", ")))
}

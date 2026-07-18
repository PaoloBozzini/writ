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
//! Covers `Int` / `Bool` / `Text`, the arithmetic / comparison / boolean
//! operators (with the interpreter's checked overflow and division-by-zero
//! semantics, and short-circuit `&&` / `||`), `let` / `if` / `return`, function
//! calls, `print`, the text built-ins (`concat` / `text_len` / `char_at` /
//! `substring`, char-based via a small UTF-8 decoder), the file-I/O built-ins
//! (`read_file` / `write_file`), sum-type constructors and
//! `match` (including nested sub-patterns), capabilities (`Cap<..>` parameters
//! and `grant<A>(..)` narrowing), and lowered contract checks — the full surface
//! the interpreter
//! runs. Any construct the front end could add later that codegen does not yet
//! handle is reported as a [`CodegenError`] rather than mis-compiled.

use std::collections::HashMap;
use std::fmt::Write as _;

use writ_ast::{
    BinaryOp, Blame, Block, Expr, Item, LiteralKind, Module, Pattern, Span, Stmt, UnaryOp,
};

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
/// division-by-zero traps, structural equality, and the interpreter's value
/// formatting for `print`.
const PRELUDE: &str = r#"#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef enum { W_INT, W_BOOL, W_UNIT, W_TEXT, W_VARIANT, W_CAP } WTag;
typedef struct WValue WValue;
struct WValue { WTag tag; int64_t i; const char *s; WValue *fields; int64_t nfields; };

static WValue w_int(int64_t x) { WValue v; v.tag = W_INT; v.i = x; v.s = 0; v.fields = 0; v.nfields = 0; return v; }
static WValue w_bool(int b) { WValue v; v.tag = W_BOOL; v.i = b ? 1 : 0; v.s = 0; v.fields = 0; v.nfields = 0; return v; }
static WValue w_unit(void) { WValue v; v.tag = W_UNIT; v.i = 0; v.s = 0; v.fields = 0; v.nfields = 0; return v; }
static WValue w_text(const char *s) { WValue v; v.tag = W_TEXT; v.i = 0; v.s = s; v.fields = 0; v.nfields = 0; return v; }
static WValue w_cap(const char *authority) { WValue v; v.tag = W_CAP; v.i = 0; v.s = authority; v.fields = 0; v.nfields = 0; return v; }
static WValue w_variant(const char *name, WValue *fields, int64_t n) {
    WValue v; v.tag = W_VARIANT; v.i = 0; v.s = name; v.nfields = n;
    if (n > 0) { v.fields = malloc(sizeof(WValue) * (size_t) n); for (int64_t k = 0; k < n; k++) v.fields[k] = fields[k]; }
    else v.fields = 0;
    return v;
}
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
static int w_equal(WValue a, WValue b) {
    if (a.tag != b.tag) return 0;
    switch (a.tag) {
        case W_INT: case W_BOOL: case W_UNIT: return a.i == b.i;
        case W_TEXT: case W_CAP: return strcmp(a.s, b.s) == 0;
        case W_VARIANT:
            if (strcmp(a.s, b.s) != 0 || a.nfields != b.nfields) return 0;
            for (int64_t k = 0; k < a.nfields; k++) if (!w_equal(a.fields[k], b.fields[k])) return 0;
            return 1;
    }
    return 0;
}
static WValue w_eq(WValue a, WValue b) { return w_bool(w_equal(a, b)); }
static WValue w_ne(WValue a, WValue b) { return w_bool(!w_equal(a, b)); }
static int w_is(WValue v, const char *name) { return v.tag == W_VARIANT && strcmp(v.s, name) == 0; }
static void w_fprint(FILE *f, WValue v) {
    switch (v.tag) {
        case W_INT: fprintf(f, "%lld", (long long) v.i); break;
        case W_BOOL: fprintf(f, "%s", v.i ? "true" : "false"); break;
        case W_UNIT: fprintf(f, "()"); break;
        case W_TEXT: fprintf(f, "%s", v.s); break;
        case W_CAP: fprintf(f, "<capability %s>", v.s); break;
        case W_VARIANT:
            fprintf(f, "%s", v.s);
            if (v.nfields > 0) {
                fprintf(f, "(");
                for (int64_t k = 0; k < v.nfields; k++) { if (k) fprintf(f, ", "); w_fprint(f, v.fields[k]); }
                fprintf(f, ")");
            }
            break;
    }
}
static WValue w_print(WValue v) { w_fprint(stdout, v); printf("\n"); return w_unit(); }

/* Text is a sequence of Unicode scalar values (UTF-8), matching the interpreter,
   so text built-ins index by code point, not byte. */
static int64_t w_u8_step(const unsigned char *p) {
    unsigned char c = *p;
    if (c < 0x80) return 1;
    if ((c >> 5) == 0x6) return 2;
    if ((c >> 4) == 0xE) return 3;
    if ((c >> 3) == 0x1E) return 4;
    return 1;
}
static int64_t w_u8_count(const char *s) {
    int64_t n = 0; const unsigned char *p = (const unsigned char *) s;
    while (*p) { p += w_u8_step(p); n++; }
    return n;
}
/* Pointer to the i-th code point, or 0 if i is out of range. */
static const char *w_u8_at(const char *s, int64_t i) {
    if (i < 0) return 0;
    const unsigned char *p = (const unsigned char *) s;
    while (*p && i > 0) { p += w_u8_step(p); i--; }
    if (i != 0 || *p == 0) return 0;
    return (const char *) p;
}
static WValue w_text_len(WValue s) { return w_int(w_u8_count(s.s)); }
static WValue w_concat(WValue a, WValue b) {
    size_t la = strlen(a.s), lb = strlen(b.s);
    char *r = malloc(la + lb + 1);
    memcpy(r, a.s, la); memcpy(r + la, b.s, lb); r[la + lb] = 0;
    return w_text(r);
}
static WValue w_char_at(WValue s, WValue iv) {
    const char *p = w_u8_at(s.s, iv.i);
    if (!p) w_trap("char_at: index out of range");
    size_t step = (size_t) w_u8_step((const unsigned char *) p);
    char *r = malloc(step + 1);
    memcpy(r, p, step); r[step] = 0;
    return w_text(r);
}
static WValue w_substring(WValue s, WValue sv, WValue ev) {
    int64_t start = sv.i, end = ev.i, len = w_u8_count(s.s);
    if (start < 0 || end < start || end > len) w_trap("substring: range out of bounds");
    const char *ps = (start == len) ? (s.s + strlen(s.s)) : w_u8_at(s.s, start);
    const char *pe = (end == len) ? (s.s + strlen(s.s)) : w_u8_at(s.s, end);
    size_t nbytes = (size_t) (pe - ps);
    char *r = malloc(nbytes + 1);
    memcpy(r, ps, nbytes); r[nbytes] = 0;
    return w_text(r);
}

/* File I/O. The capability argument carries no runtime data (its authority was
   checked statically); it is accepted and ignored. */
static WValue w_read_file(WValue cap, WValue path) {
    (void) cap;
    FILE *f = fopen(path.s, "rb");
    if (!f) w_trap("read_file: cannot open file");
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (n < 0) { fclose(f); w_trap("read_file: cannot size file"); }
    char *buf = malloc((size_t) n + 1);
    size_t got = fread(buf, 1, (size_t) n, f);
    fclose(f);
    buf[got] = 0;
    return w_text(buf);
}
static WValue w_write_file(WValue cap, WValue path, WValue contents) {
    (void) cap;
    FILE *f = fopen(path.s, "wb");
    if (!f) w_trap("write_file: cannot open file");
    fwrite(contents.s, 1, strlen(contents.s), f);
    fclose(f);
    return w_unit();
}
"#;

/// Emit a complete C translation unit for `module`.
///
/// The module must be **checked**, **lowered** (contracts desugared into
/// `Stmt::Check`), and **linked** into a single module (no imports).
///
/// # Errors
/// Returns a [`CodegenError`] if the module uses a construct this back end does
/// not yet support, or has no `main` function.
pub fn emit_c(module: &Module) -> Result<String, CodegenError> {
    let mut ctors: HashMap<String, usize> = HashMap::new();
    for item in &module.items {
        if let Item::Type(decl) = item {
            for v in &decl.variants {
                ctors.insert(v.name.clone(), v.fields.len());
            }
        }
    }

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

    let mut e = Emitter {
        ctors,
        out: String::new(),
        counter: 0,
    };
    e.out.push_str(PRELUDE);
    e.out.push('\n');

    // Forward declarations, so call order does not matter.
    for f in &funcs {
        let _ = writeln!(
            e.out,
            "static WValue {};",
            proto(&f.signature.name, f.signature.params.len())
        );
    }
    e.out.push('\n');

    // Definitions.
    for f in &funcs {
        let _ = writeln!(
            e.out,
            "static WValue {} {{",
            proto(&f.signature.name, f.signature.params.len())
        );
        for (i, p) in f.signature.params.iter().enumerate() {
            let _ = writeln!(e.out, "    WValue {} = p{};", local(&p.name), i);
        }
        e.emit_block(&f.body, 1)?;
        // A path that falls off the end still yields a value.
        e.out.push_str("    return w_unit();\n");
        e.out.push_str("}\n\n");
    }

    // The C entry point calls Writ's `main`, handing the runtime's root
    // capability to each capability parameter (tagged with the authority it
    // grants) and a unit placeholder to anything else — mirroring how the
    // interpreter starts `main`.
    e.out.push_str("int main(void) {\n");
    let args: Vec<String> = main
        .signature
        .params
        .iter()
        .map(|p| {
            if p.ty.name == "Cap" {
                let authority = p.ty.args.first().map_or("Root", |a| a.name.as_str());
                format!("w_cap(\"{authority}\")")
            } else {
                "w_unit()".to_string()
            }
        })
        .collect();
    let _ = writeln!(
        e.out,
        "    {}({});",
        cname(&main.signature.name),
        args.join(", ")
    );
    e.out.push_str("    return 0;\n}\n");

    Ok(e.out)
}

struct Emitter {
    /// Sum-type constructor name → arity, so a name can be classified as a
    /// constructor (vs a function or a variable).
    ctors: HashMap<String, usize>,
    out: String,
    /// A monotonic counter for unique temporary names (e.g. per `match`).
    counter: usize,
}

impl Emitter {
    fn fresh(&mut self) -> usize {
        let n = self.counter;
        self.counter += 1;
        n
    }

    fn emit_block(&mut self, block: &Block, level: usize) -> Result<(), CodegenError> {
        for stmt in &block.stmts {
            self.emit_stmt(stmt, level)?;
        }
        Ok(())
    }

    fn emit_stmt(&mut self, stmt: &Stmt, level: usize) -> Result<(), CodegenError> {
        match stmt {
            Stmt::Let { name, value, .. } => {
                let v = self.emit_expr(value)?;
                indent(level, &mut self.out);
                let _ = writeln!(self.out, "WValue {} = {};", local(name), v);
            }
            Stmt::Expr(e) => {
                let v = self.emit_expr(e)?;
                indent(level, &mut self.out);
                let _ = writeln!(self.out, "{v};");
            }
            Stmt::Return { value, .. } => {
                let v = match value {
                    Some(e) => self.emit_expr(e)?,
                    None => "w_unit()".to_string(),
                };
                indent(level, &mut self.out);
                let _ = writeln!(self.out, "return {v};");
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                let c = self.emit_expr(cond)?;
                indent(level, &mut self.out);
                let _ = writeln!(self.out, "if (w_as_bool({c})) {{");
                self.emit_block(then_block, level + 1)?;
                indent(level, &mut self.out);
                self.out.push('}');
                if let Some(else_block) = else_block {
                    self.out.push_str(" else {\n");
                    self.emit_block(else_block, level + 1)?;
                    indent(level, &mut self.out);
                    self.out.push('}');
                }
                self.out.push('\n');
            }
            // A lowered contract check. Trapping reproduces the interpreter's
            // exact message, so runtime failures read identically across engines.
            Stmt::Check {
                predicate, blame, ..
            } => {
                let p = self.emit_expr(predicate)?;
                let msg = match blame {
                    Blame::Caller => "precondition violated (blame: caller)",
                    Blame::Implementation => "postcondition violated (blame: implementation)",
                };
                indent(level, &mut self.out);
                let _ = writeln!(self.out, "if (!w_as_bool({p})) w_trap(\"{msg}\");");
            }
        }
        Ok(())
    }

    fn emit_expr(&mut self, expr: &Expr) -> Result<String, CodegenError> {
        match expr {
            Expr::Literal(lit) => match &lit.kind {
                LiteralKind::Int(n) => Ok(format!("w_int({n})")),
                LiteralKind::Bool(b) => Ok(format!("w_bool({})", i32::from(*b))),
                LiteralKind::Text(s) => Ok(format!("w_text(\"{}\")", c_escape(s))),
            },
            Expr::Identifier { name, span } => {
                // A bare name that is a nullary constructor is a sum-type value;
                // any other uppercase name that is not a known constructor is an
                // error; otherwise it is a local/parameter.
                if let Some(arity) = self.ctors.get(name).copied() {
                    if arity != 0 {
                        return Err(CodegenError::new(
                            *span,
                            format!("constructor `{name}` expects {arity} argument(s)"),
                        ));
                    }
                    return Ok(format!("w_variant(\"{name}\", 0, 0)"));
                }
                Ok(local(name))
            }
            Expr::Unary { op, operand, .. } => {
                let inner = self.emit_expr(operand)?;
                Ok(match op {
                    UnaryOp::Neg => format!("w_neg({inner})"),
                    UnaryOp::Not => format!("w_not({inner})"),
                })
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let l = self.emit_expr(left)?;
                let r = self.emit_expr(right)?;
                Ok(match op {
                    // Short-circuit operators emit C `&&` / `||` so a
                    // side-effecting right operand is skipped as the interpreter
                    // skips it.
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
                callee,
                type_args,
                args,
                span,
            } => self.emit_call(callee, type_args, args, *span),
            Expr::Match {
                scrutinee,
                arms,
                span,
            } => self.emit_match(scrutinee, arms, *span),
            Expr::Member { span, .. } => Err(CodegenError::new(
                *span,
                "member access is not supported by the C back end yet",
            )),
        }
    }

    fn emit_call(
        &mut self,
        callee: &Expr,
        type_args: &[writ_ast::TypeExpr],
        args: &[Expr],
        span: Span,
    ) -> Result<String, CodegenError> {
        let Expr::Identifier { name, .. } = callee else {
            return Err(CodegenError::new(
                span,
                "only named functions can be called in the C back end yet",
            ));
        };
        let emitted: Vec<String> = args
            .iter()
            .map(|a| self.emit_expr(a))
            .collect::<Result<_, _>>()?;

        // A constructor call builds a sum-type value.
        if let Some(arity) = self.ctors.get(name).copied() {
            if emitted.len() != arity {
                return Err(CodegenError::new(
                    span,
                    format!("constructor `{name}` expects {arity} argument(s)"),
                ));
            }
            return Ok(format!(
                "w_variant(\"{name}\", (WValue[]){{{}}}, {})",
                emitted.join(", "),
                emitted.len()
            ));
        }

        // Built-ins. `print` writes a line; `sanitize` is a runtime identity
        // (taint is a compile-time property with no runtime representation).
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
        // Text and file-I/O built-ins map to runtime helpers.
        if let Some(helper) = match name.as_str() {
            "concat" => Some(("w_concat", 2)),
            "text_len" => Some(("w_text_len", 1)),
            "char_at" => Some(("w_char_at", 2)),
            "substring" => Some(("w_substring", 3)),
            "read_file" => Some(("w_read_file", 2)),
            "write_file" => Some(("w_write_file", 3)),
            _ => None,
        } {
            let (func, arity) = helper;
            if emitted.len() != arity {
                return Err(CodegenError::new(
                    span,
                    format!("`{name}` expects {arity} argument(s)"),
                ));
            }
            return Ok(format!("{func}({})", emitted.join(", ")));
        }
        // `grant<A>(cap)` narrows authority to `A`. Capabilities carry no
        // runtime effect (there are no effectful built-ins), so it just yields
        // an opaque capability tagged with the authority `A` — matching the
        // interpreter. The source capability argument is a pure reference and
        // need not be re-evaluated.
        if name == "grant" {
            let authority = type_args
                .first()
                .map(|t| t.name.as_str())
                .ok_or_else(|| CodegenError::new(span, "`grant` needs one type argument"))?;
            return Ok(format!("w_cap(\"{authority}\")"));
        }
        Ok(format!("{}({})", cname(name), emitted.join(", ")))
    }

    /// Emit a `match` as a GNU statement-expression: bind the scrutinee once,
    /// then a chain of `if`/`else if` — one per arm — assigns the matching arm's
    /// body to a result temporary. Exhaustiveness is already checked statically;
    /// the trailing `else` traps to mirror the interpreter's runtime error.
    fn emit_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[writ_ast::MatchArm],
        _span: Span,
    ) -> Result<String, CodegenError> {
        let id = self.fresh();
        let scrut = format!("_m{id}_s");
        let res = format!("_m{id}_r");
        let scrut_expr = self.emit_expr(scrutinee)?;

        let mut body = String::new();
        let _ = write!(body, "WValue {scrut} = {scrut_expr}; WValue {res}; ");

        for (i, arm) in arms.iter().enumerate() {
            let keyword = if i == 0 { "if" } else { "else if" };
            let (cond, bindings) = self.pattern_test(&arm.pattern, &scrut)?;
            let arm_body = self.emit_expr(&arm.body)?;
            let _ = write!(
                body,
                "{keyword} ({cond}) {{ {bindings}{res} = {arm_body}; }} "
            );
        }
        let _ = write!(body, "else {{ w_trap(\"no matching arm\"); }} {res};");

        Ok(format!("({{ {body} }})"))
    }

    /// The C test for a top-level pattern against `scrut`, plus any C binding
    /// statements it introduces. A wildcard or a plain identifier matches
    /// anything; a variant (or a nullary-constructor identifier) tests the tag.
    fn pattern_test(
        &self,
        pattern: &Pattern,
        scrut: &str,
    ) -> Result<(String, String), CodegenError> {
        match pattern {
            Pattern::Wildcard { .. } => Ok(("1".to_string(), String::new())),
            Pattern::Ident { name, span } => {
                if self.ctors.get(name).copied() == Some(0) {
                    // A nullary constructor, e.g. `None`.
                    Ok((format!("w_is({scrut}, \"{name}\")"), String::new()))
                } else if self.ctors.contains_key(name) {
                    Err(CodegenError::new(
                        *span,
                        format!("constructor `{name}` used without its arguments in a pattern"),
                    ))
                } else {
                    // A binding matches anything and names the whole value.
                    Ok((
                        "1".to_string(),
                        format!("WValue {} = {scrut}; ", local(name)),
                    ))
                }
            }
            Pattern::Variant { name, args, .. } => {
                // Test this variant's tag, then recurse into each field: a
                // sub-pattern contributes its own tag test (folded into the arm
                // condition) and its bindings, read from `{scrut}.fields[i]`.
                let mut cond = format!("w_is({scrut}, \"{name}\")");
                let mut bindings = String::new();
                for (i, sub) in args.iter().enumerate() {
                    let field = format!("{scrut}.fields[{i}]");
                    let (subcond, subbind) = self.pattern_test(sub, &field)?;
                    if subcond != "1" {
                        cond = format!("({cond} && {subcond})");
                    }
                    bindings.push_str(&subbind);
                }
                Ok((cond, bindings))
            }
        }
    }
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

/// Escape a Writ text value for a C string literal.
fn c_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\{:03o}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn indent(level: usize, out: &mut String) {
    for _ in 0..level {
        out.push_str("    ");
    }
}

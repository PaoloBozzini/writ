//! The type-checking pass.
//!
//! Writ is strongly and statically typed with **no implicit coercions** and
//! **non-null by default** (the type system simply has no null value, so that
//! property holds by construction). A type mismatch is a compile error with an
//! exact span, not a runtime surprise.
//!
//! This pass is self-contained: it imports no other checker. It computes types
//! transiently to validate the program and never stores them back onto the AST.

use std::collections::HashMap;

use writ_ast::{BinaryOp, Block, Expr, Item, Module, Signature, Stmt, UnaryOp};
use writ_ast::{Diagnostic, Span};

use crate::ty::Type;

/// The built-in `print` accepts one argument of any type and returns `Unit`.
const PRINT: &str = "print";

/// Type-check a module, returning all type diagnostics in source order. An empty
/// result means the module is well-typed.
#[must_use]
pub fn check_types(module: &Module) -> Vec<Diagnostic> {
    let mut checker = Checker::new(module);
    for item in &module.items {
        let Item::Function(f) = item else {
            continue;
        };
        checker.check_function(&f.signature, &f.body);
    }
    checker.diagnostics
}

/// A function's resolved parameter and return types.
struct FnSig {
    params: Vec<Type>,
    ret: Type,
}

fn signature_types(sig: &Signature) -> FnSig {
    FnSig {
        params: sig.params.iter().map(|p| Type::resolve(&p.ty)).collect(),
        ret: sig.return_type.as_ref().map_or(Type::Unit, Type::resolve),
    }
}

struct Checker<'m> {
    funcs: HashMap<&'m str, FnSig>,
    diagnostics: Vec<Diagnostic>,
    /// The declared return type of the function currently being checked.
    current_ret: Type,
    /// Variable scopes for the current function, innermost last.
    scopes: Vec<HashMap<String, Type>>,
}

impl<'m> Checker<'m> {
    fn new(module: &'m Module) -> Self {
        let mut funcs = HashMap::new();
        for item in &module.items {
            let Item::Function(f) = item else {
                continue;
            };
            // A duplicate name is reported elsewhere; keep the first here.
            funcs
                .entry(f.signature.name.as_str())
                .or_insert_with(|| signature_types(&f.signature));
        }
        Self {
            funcs,
            diagnostics: Vec::new(),
            current_ret: Type::Unit,
            scopes: Vec::new(),
        }
    }

    fn error(&mut self, code: &str, span: Span, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::error(code, span, message));
    }

    fn check_function(&mut self, sig: &Signature, body: &Block) {
        self.current_ret = sig.return_type.as_ref().map_or(Type::Unit, Type::resolve);
        self.scopes = vec![HashMap::new()];
        for param in &sig.params {
            self.bind(param.name.clone(), Type::resolve(&param.ty));
        }
        self.check_block(body);
    }

    fn bind(&mut self, name: String, ty: Type) {
        self.scopes.last_mut().expect("a scope").insert(name, ty);
    }

    fn lookup(&self, name: &str) -> Option<Type> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    fn check_block(&mut self, block: &Block) {
        self.scopes.push(HashMap::new());
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
        self.scopes.pop();
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let {
                name, ty, value, ..
            } => {
                let value_ty = self.check_expr(value);
                let bound = match ty {
                    Some(annot) => {
                        let declared = Type::resolve(annot);
                        if !declared.compatible(&value_ty) {
                            self.error(
                                "T0001",
                                value.span(),
                                format!(
                                    "type mismatch: binding `{name}` is `{declared}` but its value is `{value_ty}`"
                                ),
                            );
                        }
                        declared
                    }
                    None => value_ty,
                };
                self.bind(name.clone(), bound);
            }
            Stmt::Expr(expr) => {
                self.check_expr(expr);
            }
            Stmt::Return { value, span } => {
                let actual = match value {
                    Some(expr) => self.check_expr(expr),
                    None => Type::Unit,
                };
                if !actual.compatible(&self.current_ret) {
                    let expected = self.current_ret.clone();
                    self.error(
                        "T0005",
                        *span,
                        format!("return type mismatch: expected `{expected}`, found `{actual}`"),
                    );
                }
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                let cond_ty = self.check_expr(cond);
                if !cond_ty.compatible(&Type::Bool) {
                    self.error(
                        "T0001",
                        cond.span(),
                        format!("`if` condition must be `Bool`, found `{cond_ty}`"),
                    );
                }
                self.check_block(then_block);
                if let Some(else_block) = else_block {
                    self.check_block(else_block);
                }
            }
        }
    }

    fn check_expr(&mut self, expr: &Expr) -> Type {
        match expr {
            Expr::Literal(lit) => match lit.kind {
                writ_ast::LiteralKind::Int(_) => Type::Int,
                writ_ast::LiteralKind::Bool(_) => Type::Bool,
                writ_ast::LiteralKind::Text(_) => Type::Text,
            },
            Expr::Identifier { name, span } => self.lookup(name).unwrap_or_else(|| {
                self.error("T0002", *span, format!("unknown variable `{name}`"));
                Type::Error
            }),
            Expr::Unary { op, operand, span } => {
                let ty = self.check_expr(operand);
                match op {
                    UnaryOp::Neg => {
                        self.require(&ty, &Type::Int, *span, "operand of `-`");
                        Type::Int
                    }
                    UnaryOp::Not => {
                        self.require(&ty, &Type::Bool, *span, "operand of `!`");
                        Type::Bool
                    }
                }
            }
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => {
                let lt = self.check_expr(left);
                let rt = self.check_expr(right);
                self.check_binary(*op, &lt, &rt, *span)
            }
            Expr::Call { callee, args, span } => self.check_call(callee, args, *span),
            // Type-checking of `match` (variant resolution + exhaustiveness) is
            // handled by the sum-type checker pass; treat it as unknown here.
            Expr::Match { .. } => Type::Error,
        }
    }

    fn check_binary(&mut self, op: BinaryOp, lt: &Type, rt: &Type, span: Span) -> Type {
        use BinaryOp::*;
        match op {
            Add | Sub | Mul | Div | Rem => {
                self.require(lt, &Type::Int, span, "left operand");
                self.require(rt, &Type::Int, span, "right operand");
                Type::Int
            }
            Lt | Le | Gt | Ge => {
                self.require(lt, &Type::Int, span, "left operand");
                self.require(rt, &Type::Int, span, "right operand");
                Type::Bool
            }
            Eq | Ne => {
                if !lt.compatible(rt) {
                    self.error(
                        "T0001",
                        span,
                        format!(
                            "cannot compare `{lt}` with `{rt}`: operands must have the same type"
                        ),
                    );
                }
                Type::Bool
            }
            And | Or => {
                self.require(lt, &Type::Bool, span, "left operand");
                self.require(rt, &Type::Bool, span, "right operand");
                Type::Bool
            }
        }
    }

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Type {
        let arg_types: Vec<Type> = args.iter().map(|a| self.check_expr(a)).collect();

        let Expr::Identifier { name, .. } = callee else {
            self.error("T0003", span, "only named functions can be called");
            return Type::Error;
        };

        if name == PRINT && !self.funcs.contains_key(name.as_str()) {
            if arg_types.len() != 1 {
                self.error(
                    "T0004",
                    span,
                    format!("`print` expects 1 argument, found {}", arg_types.len()),
                );
            }
            return Type::Unit;
        }

        let Some(sig) = self.funcs.get(name.as_str()) else {
            self.error("T0003", span, format!("unknown function `{name}`"));
            return Type::Error;
        };
        let ret = sig.ret.clone();
        let params = sig.params.clone();

        if arg_types.len() != params.len() {
            self.error(
                "T0004",
                span,
                format!(
                    "function `{name}` expects {} argument(s), found {}",
                    params.len(),
                    arg_types.len()
                ),
            );
        }
        for (i, (param_ty, arg_ty)) in params.iter().zip(&arg_types).enumerate() {
            if !param_ty.compatible(arg_ty) {
                self.error(
                    "T0001",
                    args[i].span(),
                    format!(
                        "argument {} to `{name}`: expected `{param_ty}`, found `{arg_ty}`",
                        i + 1
                    ),
                );
            }
        }
        ret
    }

    /// Require `actual` to be compatible with `expected`, reporting a mismatch.
    fn require(&mut self, actual: &Type, expected: &Type, span: Span, what: &str) {
        if !actual.compatible(expected) {
            self.error(
                "T0001",
                span,
                format!("{what} must be `{expected}`, found `{actual}`"),
            );
        }
    }
}

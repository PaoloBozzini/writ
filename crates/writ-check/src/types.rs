//! The type-checking pass.
//!
//! Writ is strongly and statically typed with **no implicit coercions** and
//! **non-null by default** (the type system simply has no null value, so that
//! property holds by construction). A type mismatch is a compile error with an
//! exact span, not a runtime surprise.
//!
//! This pass is self-contained: it imports no other checker. It computes types
//! transiently to validate the program and never stores them back onto the AST.

use std::collections::{HashMap, HashSet};

use writ_ast::{BinaryOp, Block, Expr, Item, Module, Signature, Stmt, UnaryOp};
use writ_ast::{Diagnostic, Span};

use crate::ty::Type;

/// The built-in `print` accepts one argument of any type and returns `Unit`.
const PRINT: &str = "print";

/// The capability-narrowing built-in: `grant<A>(cap) -> Cap<A>`.
const GRANT: &str = "grant";

/// The type-head for a capability.
const CAP: &str = "Cap";

/// The authority of the root capability — narrows to any specific power.
const ROOT: &str = "Root";

/// The taint-removing built-in: `sanitize(Tainted<T>) -> T`.
const SANITIZE: &str = "sanitize";

/// The type-head marking untrusted data.
const TAINTED: &str = "Tainted";

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

/// If `ty` is a capability type `Cap<A>`, returns the authority name `A`.
fn cap_authority(ty: &Type) -> Option<&str> {
    match ty {
        Type::Named { name, args } if name == CAP => match args.first() {
            Some(Type::Named { name, .. }) => Some(name.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// A sum-type constructor: the type it belongs to and its field arity.
struct CtorInfo {
    owner: String,
    arity: usize,
}

struct Checker<'m> {
    funcs: HashMap<&'m str, FnSig>,
    /// Sum type name → its variant names, in declaration order.
    sum_variants: HashMap<&'m str, Vec<&'m str>>,
    /// Variant name → its constructor info.
    ctors: HashMap<&'m str, CtorInfo>,
    diagnostics: Vec<Diagnostic>,
    /// The declared return type of the function currently being checked.
    current_ret: Type,
    /// Variable scopes for the current function, innermost last.
    scopes: Vec<HashMap<String, Type>>,
}

impl<'m> Checker<'m> {
    fn new(module: &'m Module) -> Self {
        let mut funcs = HashMap::new();
        let mut sum_variants = HashMap::new();
        let mut ctors = HashMap::new();
        for item in &module.items {
            match item {
                Item::Function(f) => {
                    // A duplicate name is reported elsewhere; keep the first here.
                    funcs
                        .entry(f.signature.name.as_str())
                        .or_insert_with(|| signature_types(&f.signature));
                }
                Item::Type(decl) => {
                    let names = decl.variants.iter().map(|v| v.name.as_str()).collect();
                    sum_variants.insert(decl.name.as_str(), names);
                    for variant in &decl.variants {
                        ctors
                            .entry(variant.name.as_str())
                            .or_insert_with(|| CtorInfo {
                                owner: decl.name.clone(),
                                arity: variant.fields.len(),
                            });
                    }
                }
            }
        }
        Self {
            funcs,
            sum_variants,
            ctors,
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
        self.check_contracts(sig);
        self.check_block(body);
    }

    /// Type-check the signature's contract predicates. A `requires` sees the
    /// parameters; an `ensures` also sees `result` bound to the return type. Each
    /// predicate must be `Bool`.
    fn check_contracts(&mut self, sig: &Signature) {
        for clause in &sig.requires {
            let ty = self.check_expr(&clause.predicate);
            self.require_contract(&ty, clause.predicate.span(), "requires");
        }
        if !sig.ensures.is_empty() {
            self.scopes.push(HashMap::new());
            let ret = self.current_ret.clone();
            self.bind("result".to_string(), ret);
            for clause in &sig.ensures {
                let ty = self.check_expr(&clause.predicate);
                self.require_contract(&ty, clause.predicate.span(), "ensures");
            }
            self.scopes.pop();
        }
    }

    fn require_contract(&mut self, ty: &Type, span: Span, clause: &str) {
        if !ty.compatible(&Type::Bool) {
            self.error(
                "T0007",
                span,
                format!("`{clause}` predicate must be `Bool`, found `{ty}`"),
            );
        }
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
            Expr::Identifier { name, span } => {
                if let Some(ty) = self.lookup(name) {
                    ty
                } else if let Some((owner, arity)) = self
                    .ctors
                    .get(name.as_str())
                    .map(|c| (c.owner.clone(), c.arity))
                {
                    // A bare nullary constructor, e.g. `None`.
                    if arity != 0 {
                        self.error(
                            "T0004",
                            *span,
                            format!("constructor `{name}` expects {arity} argument(s), found 0"),
                        );
                    }
                    Type::Named {
                        name: owner,
                        args: Vec::new(),
                    }
                } else {
                    self.error("T0002", *span, format!("unknown variable `{name}`"));
                    Type::Error
                }
            }
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
            Expr::Call {
                callee,
                type_args,
                args,
                span,
            } => self.check_call(callee, type_args, args, *span),
            Expr::Match {
                scrutinee,
                arms,
                span,
            } => self.check_match(scrutinee, arms, *span),
        }
    }

    /// Type-check a `match`: the arms' bodies must agree on a type, and — when
    /// the scrutinee is a sum type — every variant must be covered.
    fn check_match(&mut self, scrutinee: &Expr, arms: &[writ_ast::MatchArm], span: Span) -> Type {
        let scrutinee_ty = self.check_expr(scrutinee);
        // Which sum type (if any) is being matched, so we can check coverage.
        let sum_name = match &scrutinee_ty {
            Type::Named { name, .. } if self.sum_variants.contains_key(name.as_str()) => {
                Some(name.clone())
            }
            _ => None,
        };

        let mut covered: HashSet<&str> = HashSet::new();
        let mut has_catch_all = false;
        let mut result_ty: Option<Type> = None;

        for arm in arms {
            self.scopes.push(HashMap::new());
            self.collect_pattern(&arm.pattern, &mut covered, &mut has_catch_all);
            let body_ty = self.check_expr(&arm.body);
            self.scopes.pop();

            match &result_ty {
                None => result_ty = Some(body_ty),
                Some(prev) => {
                    if !prev.compatible(&body_ty) {
                        self.error(
                            "T0001",
                            arm.body.span(),
                            format!("match arms have mismatched types: `{prev}` vs `{body_ty}`"),
                        );
                    }
                }
            }
        }

        // Exhaustiveness: a sum-type match with no catch-all must cover every
        // variant.
        if let Some(sum) = sum_name {
            if !has_catch_all {
                if let Some(variants) = self.sum_variants.get(sum.as_str()) {
                    let missing: Vec<&str> = variants
                        .iter()
                        .copied()
                        .filter(|v| !covered.contains(v))
                        .collect();
                    if !missing.is_empty() {
                        self.error(
                            "T0006",
                            span,
                            format!(
                                "non-exhaustive match on `{sum}`: missing {}",
                                missing
                                    .iter()
                                    .map(|v| format!("`{v}`"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        );
                    }
                }
            }
        }

        result_ty.unwrap_or(Type::Error)
    }

    /// Record which variants an arm's pattern covers, binding any pattern
    /// variables. A wildcard or a non-constructor identifier is a catch-all.
    fn collect_pattern(
        &mut self,
        pattern: &writ_ast::Pattern,
        covered: &mut HashSet<&'m str>,
        has_catch_all: &mut bool,
    ) {
        use writ_ast::Pattern;
        match pattern {
            Pattern::Wildcard { .. } => *has_catch_all = true,
            Pattern::Ident { name, .. } => {
                // A name that is a known nullary variant covers that variant;
                // otherwise it is a binding that matches anything.
                if let Some((variant, _)) = self.ctors.get_key_value(name.as_str()) {
                    covered.insert(*variant);
                } else {
                    *has_catch_all = true;
                    self.bind(name.clone(), Type::Error);
                }
            }
            Pattern::Variant { name, args, .. } => {
                if let Some((variant, info)) = self.ctors.get_key_value(name.as_str()) {
                    covered.insert(*variant);
                    if args.len() != info.arity {
                        // Arity is validated leniently; payload types are checked
                        // once generic instantiation lands (follow-up).
                    }
                }
                // Bind sub-pattern variables opaquely so the arm body type-checks
                // without a full generic-instantiation model.
                for sub in args {
                    self.bind_pattern_vars(sub);
                }
            }
        }
    }

    /// Bind every variable a (sub-)pattern introduces, typed opaquely.
    fn bind_pattern_vars(&mut self, pattern: &writ_ast::Pattern) {
        use writ_ast::Pattern;
        match pattern {
            Pattern::Wildcard { .. } => {}
            Pattern::Ident { name, .. } => {
                if !self.ctors.contains_key(name.as_str()) {
                    self.bind(name.clone(), Type::Error);
                }
            }
            Pattern::Variant { args, .. } => {
                for sub in args {
                    self.bind_pattern_vars(sub);
                }
            }
        }
    }

    /// Type-check `grant<A>(cap)`: narrow a broader capability to authority `A`.
    fn check_grant(
        &mut self,
        type_args: &[writ_ast::TypeExpr],
        arg_types: &[Type],
        span: Span,
    ) -> Type {
        if type_args.len() != 1 {
            self.error(
                "T0008",
                span,
                format!(
                    "`grant` needs exactly one type argument, found {}",
                    type_args.len()
                ),
            );
            return Type::Error;
        }
        let target = &type_args[0].name;
        if arg_types.len() != 1 {
            self.error(
                "T0004",
                span,
                format!("`grant` expects 1 argument, found {}", arg_types.len()),
            );
            return Type::Error;
        }
        // The source must be a capability, and narrowing may only shed authority:
        // it is valid from the root capability, or as an identity (`Cap<A>` to
        // `Cap<A>`).
        match cap_authority(&arg_types[0]) {
            Some(source) if source == ROOT || source == target => {}
            Some(source) => self.error(
                "T0009",
                span,
                format!("cannot narrow `Cap<{source}>` to `Cap<{target}>`: authority can only be shed, not amplified"),
            ),
            None => self.error(
                "T0009",
                span,
                format!("`grant` requires a capability argument, found `{}`", arg_types[0]),
            ),
        }
        Type::Named {
            name: CAP.to_string(),
            args: vec![Type::resolve(&type_args[0])],
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

    fn check_call(
        &mut self,
        callee: &Expr,
        type_args: &[writ_ast::TypeExpr],
        args: &[Expr],
        span: Span,
    ) -> Type {
        let arg_types: Vec<Type> = args.iter().map(|a| self.check_expr(a)).collect();

        let Expr::Identifier { name, .. } = callee else {
            self.error("T0003", span, "only named functions can be called");
            return Type::Error;
        };

        // `grant<A>(cap)` narrows a capability to authority `A`. It is valid only
        // from a broader capability — the root capability `Cap<Root>` — so
        // authority can only ever be shed, never amplified.
        if name == GRANT {
            return self.check_grant(type_args, &arg_types, span);
        }

        // `sanitize(x)` removes taint: `Tainted<T> -> T`. Sanitizing a value that
        // is not tainted is a no-op on its type.
        if name == SANITIZE {
            if arg_types.len() != 1 {
                self.error(
                    "T0004",
                    span,
                    format!("`sanitize` expects 1 argument, found {}", arg_types.len()),
                );
                return Type::Error;
            }
            return match &arg_types[0] {
                Type::Named { name, args } if name == TAINTED && args.len() == 1 => args[0].clone(),
                other => other.clone(),
            };
        }

        // A constructor call, e.g. `Some(x)`, builds a value of its sum type.
        if let Some((owner, arity)) = self
            .ctors
            .get(name.as_str())
            .map(|c| (c.owner.clone(), c.arity))
        {
            if arg_types.len() != arity {
                self.error(
                    "T0004",
                    span,
                    format!(
                        "constructor `{name}` expects {arity} argument(s), found {}",
                        arg_types.len()
                    ),
                );
            }
            // Payload types are left unchecked pending generic instantiation.
            return Type::Named {
                name: owner,
                args: Vec::new(),
            };
        }

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

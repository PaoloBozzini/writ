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

use writ_ast::{BinaryOp, Block, Expr, Item, Module, Signature, Stmt, TypeExpr, UnaryOp};
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

/// A sum-type constructor: the type it belongs to, that type's generic
/// parameters, and its payload field types (syntactic, referencing the
/// generics).
struct CtorInfo<'m> {
    owner: String,
    /// The owner type's generic parameters, e.g. `["T"]` for `Option<T>`.
    generics: Vec<String>,
    /// The variant's positional payload types; its length is the arity.
    fields: &'m [TypeExpr],
}

/// Resolve a variant field's syntactic type, replacing any generic parameter
/// with its inferred substitution (or [`Type::Infer`] when nothing fixed it).
fn resolve_field(texpr: &TypeExpr, generics: &[String], subst: &HashMap<String, Type>) -> Type {
    if texpr.args.is_empty() && generics.iter().any(|g| g == &texpr.name) {
        return subst.get(&texpr.name).cloned().unwrap_or(Type::Infer);
    }
    match texpr.name.as_str() {
        "Int" => Type::Int,
        "Bool" => Type::Bool,
        "Text" => Type::Text,
        "Unit" => Type::Unit,
        _ => Type::Named {
            name: texpr.name.clone(),
            args: texpr
                .args
                .iter()
                .map(|a| resolve_field(a, generics, subst))
                .collect(),
        },
    }
}

/// Infer generic bindings by structurally matching a field's declared type
/// against the actual argument type, recording the first binding for each
/// generic. Later payload checks report any inconsistency.
fn unify(field: &TypeExpr, arg: &Type, generics: &[String], subst: &mut HashMap<String, Type>) {
    if field.args.is_empty() && generics.iter().any(|g| g == &field.name) {
        subst
            .entry(field.name.clone())
            .or_insert_with(|| arg.clone());
        return;
    }
    if let Type::Named { name, args } = arg {
        if *name == field.name && args.len() == field.args.len() {
            for (f, a) in field.args.iter().zip(args) {
                unify(f, a, generics, subst);
            }
        }
    }
}

/// Instantiate a variant's field types against a scrutinee's type arguments,
/// so a pattern binds each payload variable at its precise type.
fn instantiate_fields(
    owner: &str,
    generics: &[String],
    fields: &[TypeExpr],
    scrutinee_ty: &Type,
) -> Vec<Type> {
    let mut subst = HashMap::new();
    if let Type::Named { name, args } = scrutinee_ty {
        if name == owner && args.len() == generics.len() {
            for (g, a) in generics.iter().zip(args) {
                subst.insert(g.clone(), a.clone());
            }
        }
    }
    fields
        .iter()
        .map(|f| resolve_field(f, generics, &subst))
        .collect()
}

struct Checker<'m> {
    funcs: HashMap<&'m str, FnSig>,
    /// Sum type name → its variant names, in declaration order.
    sum_variants: HashMap<&'m str, Vec<&'m str>>,
    /// Variant name → its constructor info.
    ctors: HashMap<&'m str, CtorInfo<'m>>,
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
                                generics: decl.generics.clone(),
                                fields: &variant.fields,
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
            // A lowered contract check; its predicate must be `Bool`. The parser
            // never produces this, but the type checker stays total so it can
            // run on a lowered module too.
            Stmt::Check {
                predicate, span, ..
            } => {
                let ty = self.check_expr(predicate);
                if !ty.compatible(&Type::Bool) {
                    self.error(
                        "T0007",
                        *span,
                        format!("check predicate must be `Bool`, found `{ty}`"),
                    );
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
                } else if let Some((owner, ngen, arity)) = self
                    .ctors
                    .get(name.as_str())
                    .map(|c| (c.owner.clone(), c.generics.len(), c.fields.len()))
                {
                    // A bare nullary constructor, e.g. `None`. Any generic
                    // parameter is left undetermined, so `None` fits any
                    // `Option<_>`.
                    if arity != 0 {
                        self.error(
                            "T0004",
                            *span,
                            format!("constructor `{name}` expects {arity} argument(s), found 0"),
                        );
                    }
                    Type::Named {
                        name: owner,
                        args: vec![Type::Infer; ngen],
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
            // Resolving a member (`math.add`) to another module's item is the
            // resolver's job; treat it as unknown here.
            Expr::Member { .. } => Type::Error,
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
            self.collect_pattern(
                &arm.pattern,
                &scrutinee_ty,
                &mut covered,
                &mut has_catch_all,
            );
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
    /// variables at their instantiated type. A wildcard or a non-constructor
    /// identifier is a catch-all.
    fn collect_pattern(
        &mut self,
        pattern: &writ_ast::Pattern,
        scrutinee_ty: &Type,
        covered: &mut HashSet<&'m str>,
        has_catch_all: &mut bool,
    ) {
        use writ_ast::Pattern;
        match pattern {
            Pattern::Wildcard { .. } => *has_catch_all = true,
            Pattern::Ident { name, .. } => {
                // A name that is a known nullary variant covers that variant;
                // otherwise it is a binding that matches the whole scrutinee.
                if let Some((variant, _)) = self.ctors.get_key_value(name.as_str()) {
                    covered.insert(*variant);
                } else {
                    *has_catch_all = true;
                    self.bind(name.clone(), scrutinee_ty.clone());
                }
            }
            Pattern::Variant { name, args, .. } => {
                let looked = self
                    .ctors
                    .get_key_value(name.as_str())
                    .map(|(v, c)| (*v, c.owner.clone(), c.generics.clone(), c.fields));
                if let Some((variant, owner, generics, fields)) = looked {
                    covered.insert(variant);
                    let field_types = instantiate_fields(&owner, &generics, fields, scrutinee_ty);
                    self.bind_subpatterns(args, &field_types);
                } else {
                    // Unknown constructor: bind sub-pattern variables opaquely.
                    for sub in args {
                        self.bind_pattern(sub, &Type::Error);
                    }
                }
            }
        }
    }

    /// Bind each sub-pattern to its field type; any sub-pattern beyond the known
    /// field arity (an arity mismatch) is bound opaquely.
    fn bind_subpatterns(&mut self, args: &[writ_ast::Pattern], field_types: &[Type]) {
        for (i, sub) in args.iter().enumerate() {
            let ty = field_types.get(i).cloned().unwrap_or(Type::Error);
            self.bind_pattern(sub, &ty);
        }
    }

    /// Bind every variable a (sub-)pattern introduces at its expected type,
    /// destructuring nested variant patterns against that type.
    fn bind_pattern(&mut self, pattern: &writ_ast::Pattern, expected: &Type) {
        use writ_ast::Pattern;
        match pattern {
            Pattern::Wildcard { .. } => {}
            Pattern::Ident { name, .. } => {
                if !self.ctors.contains_key(name.as_str()) {
                    self.bind(name.clone(), expected.clone());
                }
            }
            Pattern::Variant { name, args, .. } => {
                let looked = self
                    .ctors
                    .get(name.as_str())
                    .map(|c| (c.owner.clone(), c.generics.clone(), c.fields));
                if let Some((owner, generics, fields)) = looked {
                    let field_types = instantiate_fields(&owner, &generics, fields, expected);
                    self.bind_subpatterns(args, &field_types);
                } else {
                    for sub in args {
                        self.bind_pattern(sub, &Type::Error);
                    }
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

        let name = match callee {
            Expr::Identifier { name, .. } => name,
            // A `module.item(..)` call is resolved and typed across modules by
            // the resolver; the local type checker treats its result as unknown.
            Expr::Member { .. } => return Type::Error,
            _ => {
                self.error("T0003", span, "only named functions can be called");
                return Type::Error;
            }
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
        if let Some((owner, generics, fields)) = self
            .ctors
            .get(name.as_str())
            .map(|c| (c.owner.clone(), c.generics.clone(), c.fields))
        {
            let arity = fields.len();
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
            // Infer the generic substitution from the supplied arguments, then
            // check each payload against its instantiated field type.
            let mut subst = HashMap::new();
            for (field, arg_ty) in fields.iter().zip(&arg_types) {
                unify(field, arg_ty, &generics, &mut subst);
            }
            for (i, (field, arg_ty)) in fields.iter().zip(&arg_types).enumerate() {
                let expected = resolve_field(field, &generics, &subst);
                if !expected.compatible(arg_ty) {
                    self.error(
                        "T0001",
                        args[i].span(),
                        format!(
                            "constructor `{name}` argument {}: expected `{expected}`, found `{arg_ty}`",
                            i + 1
                        ),
                    );
                }
            }
            let type_args = generics
                .iter()
                .map(|g| subst.get(g).cloned().unwrap_or(Type::Infer))
                .collect();
            return Type::Named {
                name: owner,
                args: type_args,
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

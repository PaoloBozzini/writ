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

/// The name contract lowering binds to a function's returned value for its
/// `ensures` predicate — reserved from user bindings in an `ensures` function.
const RESULT: &str = "result";

/// Type-check a module, returning all type diagnostics in source order. An empty
/// result means the module is well-typed.
#[must_use]
pub fn check_types(module: &Module) -> Vec<Diagnostic> {
    let mut diagnostics = check_duplicate_names(module);
    diagnostics.extend(check_main_signature(module));
    let mut checker = Checker::new(module);
    for item in &module.items {
        let Item::Function(f) = item else {
            continue;
        };
        checker.check_function(&f.signature, &f.body);
    }
    diagnostics.append(&mut checker.diagnostics);
    diagnostics
}

/// `main` is the entry point: the runtime hands it the root capability for each
/// capability parameter, and `Unit` for anything else — so a non-capability
/// `main` parameter would silently receive a meaningless `Unit`. Refuse it
/// instead: `main` may take only capability parameters (`T0014`).
fn check_main_signature(module: &Module) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in &module.items {
        let Item::Function(f) = item else { continue };
        if f.signature.name != "main" {
            continue;
        }
        for param in &f.signature.params {
            if param.ty.name != CAP {
                diagnostics.push(Diagnostic::error(
                    "T0014",
                    param.span,
                    format!(
                        "parameter `{}` of `main` must be a capability: `main` receives only capabilities from the runtime, found `{}`",
                        param.name, param.ty.name
                    ),
                ));
            }
        }
    }
    diagnostics
}

/// Report duplicate top-level names statically: two functions with the same
/// name (`T0010`), two sum-type variants with the same name (`T0011`), or a
/// function whose name collides with a visible variant constructor (`T0019`).
/// A duplicated name is a classic generation slip; because functions and
/// nullary constructors share call syntax (`Some(x)`, `None`), a function
/// named after a constructor — including a **prelude** one like `Some` — makes
/// ambiguous call sites. The prime directive says it should be a compile error,
/// not a runtime surprise. The diagnostic points at the **second** definition.
fn check_duplicate_names(module: &Module) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut seen_fns: HashSet<&str> = HashSet::new();
    let mut seen_variants: HashSet<&str> = HashSet::new();

    // Every variant constructor name declared in the (already prelude-injected)
    // module, so a function colliding with one can be reported.
    let ctors: HashSet<&str> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Type(decl) => Some(decl.variants.iter().map(|v| v.name.as_str())),
            Item::Function(_) => None,
        })
        .flatten()
        .collect();

    for item in &module.items {
        match item {
            Item::Function(f) => {
                let name = f.signature.name.as_str();
                if !seen_fns.insert(name) {
                    diagnostics.push(Diagnostic::error(
                        "T0010",
                        f.signature.span,
                        format!("function `{name}` is defined more than once"),
                    ));
                } else if ctors.contains(name) {
                    diagnostics.push(Diagnostic::error(
                        "T0019",
                        f.signature.span,
                        format!(
                            "function `{name}` has the same name as a variant constructor: constructors and functions share call syntax, so rename one"
                        ),
                    ));
                }
            }
            Item::Type(decl) => {
                for variant in &decl.variants {
                    if !seen_variants.insert(variant.name.as_str()) {
                        diagnostics.push(Diagnostic::error(
                            "T0011",
                            variant.span,
                            format!("variant `{}` is defined more than once", variant.name),
                        ));
                    }
                }
            }
        }
    }
    diagnostics
}

/// A function's resolved parameter and return types.
struct FnSig {
    params: Vec<Type>,
    ret: Type,
    /// Whether the function performs no effects — only a pure function may be
    /// used as a first-class value, so its effects cannot escape unchecked.
    pure: bool,
}

impl FnSig {
    /// The type of this function used as a value: `fn(params) -> ret`.
    fn as_fn_type(&self) -> Type {
        Type::Fn {
            params: self.params.clone(),
            ret: Box::new(self.ret.clone()),
        }
    }
}

fn signature_types(sig: &Signature) -> FnSig {
    FnSig {
        params: sig.params.iter().map(|p| Type::resolve(&p.ty)).collect(),
        ret: sig.return_type.as_ref().map_or(Type::Unit, Type::resolve),
        pure: sig.effects.effects.is_empty(),
    }
}

/// If `ty` has no defined `==`/`!=` semantics, returns a plural noun naming its
/// kind (for the diagnostic). Functions, capabilities, and `Unit` are
/// uncomparable: the spec gives them no equality and the two engines disagree.
/// Everything else — `Int`, `Bool`, `Text`, and sum types — compares
/// structurally and identically on both engines.
fn uncomparable_kind(ty: &Type) -> Option<&'static str> {
    match ty {
        Type::Fn { .. } => Some("functions"),
        Type::Unit => Some("`Unit` values"),
        Type::Named { name, .. } if name == CAP => Some("capabilities"),
        _ => None,
    }
}

/// Whether a block guarantees a `return` on every path — i.e. control cannot
/// fall off its end. True iff some statement definitely returns (statements
/// after it are unreachable, which is fine for this liveness question).
///
/// The only surface constructs that return are `return` itself and an `if`
/// whose `then` **and** `else` branches both always return. A `match` used as a
/// statement never returns: its arm bodies are expressions, not statements, so
/// `return` cannot appear inside one — a tail `match` (rather than
/// `return match ...`) therefore correctly counts as a fall-off.
fn block_always_returns(block: &Block) -> bool {
    block.stmts.iter().any(stmt_always_returns)
}

fn stmt_always_returns(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return { .. } => true,
        Stmt::If {
            then_block,
            else_block: Some(else_block),
            ..
        } => block_always_returns(then_block) && block_always_returns(else_block),
        Stmt::If {
            else_block: None, ..
        }
        | Stmt::Let { .. }
        | Stmt::Expr(_)
        | Stmt::Check { .. } => false,
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
        self.check_result_reserved(sig, body);
        self.check_block(body);
        self.check_missing_return(sig, body);
    }

    /// Enforce that a function declared `-> T` (with `T` a real, non-`Unit`
    /// type) returns on **every** path. Falling off the end of such a function
    /// is a compile error (`T0016`): the interpreter would flow the last
    /// statement's value out while the C back end appends `return w_unit()`, so
    /// an unreturned path is a silent engine divergence — exactly the subtle
    /// wrongness the prime directive turns into a compile error.
    ///
    /// A `Unit` return (whether written or defaulted) needs no return, and a
    /// return type that failed to resolve (`Error`/`Infer`) is skipped so a
    /// prior diagnostic does not cascade.
    fn check_missing_return(&mut self, sig: &Signature, body: &Block) {
        let ret = sig.return_type.as_ref().map_or(Type::Unit, Type::resolve);
        if matches!(ret, Type::Unit | Type::Error | Type::Infer) {
            return;
        }
        if !block_always_returns(body) {
            self.error(
                "T0016",
                sig.span,
                format!(
                    "function `{}` may fall off the end without returning its `{ret}`: add a `return` on every path",
                    sig.name
                ),
            );
        }
    }

    /// In a function that declares `ensures`, `result` is reserved: contract
    /// lowering injects `let result = <return expr>` on every exit to bind the
    /// returned value the `ensures` predicate reads. A user parameter or local
    /// named `result` would collide with that injection and break the function
    /// differently per engine (the interpreter refuses the rebind; the C back
    /// end emits a redefinition), on checker-clean code. Refuse the collision
    /// statically (`T0018`) at the offending binding — matching the spec's
    /// documented `result` keyword.
    fn check_result_reserved(&mut self, sig: &Signature, body: &Block) {
        if sig.ensures.is_empty() {
            return;
        }
        for param in &sig.params {
            if param.name == RESULT {
                self.error(
                    "T0018",
                    param.span,
                    "`result` is reserved in a function with an `ensures` clause (it names the returned value): rename this parameter",
                );
            }
        }
        self.check_no_result_binding(&body.stmts);
    }

    /// Report every `let result` in an `ensures` function's body, recursing into
    /// `if` branches (the only other place a binding can appear).
    fn check_no_result_binding(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            match stmt {
                Stmt::Let { name, span, .. } if name == RESULT => {
                    self.error(
                        "T0018",
                        *span,
                        "`result` is reserved in a function with an `ensures` clause (it names the returned value): rename this local",
                    );
                }
                Stmt::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    self.check_no_result_binding(&then_block.stmts);
                    if let Some(else_block) = else_block {
                        self.check_no_result_binding(&else_block.stmts);
                    }
                }
                _ => {}
            }
        }
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
                } else if let Some((pure, fn_ty)) = self
                    .funcs
                    .get(name.as_str())
                    .map(|sig| (sig.pure, sig.as_fn_type()))
                {
                    // A top-level function used as a **value**. Only pure
                    // functions may be first-class: an effectful function passed
                    // as a value would let its effects be performed at a call
                    // site the honesty/authority passes cannot see.
                    if !pure {
                        self.error(
                            "T0015",
                            *span,
                            format!(
                                "function `{name}` performs effects, so it cannot be used as a value (only pure functions can)"
                            ),
                        );
                    }
                    fn_ty
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
            let mut binders = HashSet::new();
            self.check_pattern_binders(&arm.pattern, &mut binders);
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

    /// Reject a binder name that appears more than once in a single pattern
    /// (e.g. `Pair(x, x)`), which would otherwise silently keep the first
    /// binding and drop the second. Recurses through nested sub-patterns.
    fn check_pattern_binders(&mut self, pattern: &writ_ast::Pattern, seen: &mut HashSet<String>) {
        use writ_ast::Pattern;
        match pattern {
            Pattern::Wildcard { .. } => {}
            Pattern::Ident { name, span } => {
                // Only a real binding (not a nullary-variant name) introduces a
                // binder.
                if !self.ctors.contains_key(name.as_str()) && !seen.insert(name.clone()) {
                    self.error(
                        "T0013",
                        *span,
                        format!("binding `{name}` appears more than once in this pattern"),
                    );
                }
            }
            Pattern::Variant { args, .. } => {
                for sub in args {
                    self.check_pattern_binders(sub, seen);
                }
            }
        }
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
            Pattern::Ident { name, span } => {
                // A name that is a known nullary variant covers that variant;
                // otherwise it is a binding that matches the whole scrutinee.
                let looked = self
                    .ctors
                    .get_key_value(name.as_str())
                    .map(|(v, c)| (*v, c.owner.clone()));
                if let Some((variant, owner)) = looked {
                    if self.variant_matches_scrutinee(&owner, scrutinee_ty, name, *span) {
                        covered.insert(variant);
                    }
                } else {
                    *has_catch_all = true;
                    self.bind(name.clone(), scrutinee_ty.clone());
                }
            }
            Pattern::Variant { name, args, span } => {
                let looked = self
                    .ctors
                    .get_key_value(name.as_str())
                    .map(|(v, c)| (*v, c.owner.clone(), c.generics.clone(), c.fields));
                if let Some((variant, owner, generics, fields)) = looked {
                    if self.variant_matches_scrutinee(&owner, scrutinee_ty, name, *span) {
                        covered.insert(variant);
                        let field_types =
                            instantiate_fields(&owner, &generics, fields, scrutinee_ty);
                        self.bind_subpatterns(args, &field_types);
                    } else {
                        // Alien pattern: bind its variables opaquely so the arm
                        // body does not cascade into unknown-variable errors.
                        for sub in args {
                            self.bind_pattern(sub, &Type::Error);
                        }
                    }
                } else {
                    // Unknown constructor: bind sub-pattern variables opaquely.
                    for sub in args {
                        self.bind_pattern(sub, &Type::Error);
                    }
                }
            }
        }
    }

    /// Whether a variant pattern owned by `owner` may match a scrutinee of type
    /// `scrutinee_ty`. A pattern whose owner is not the scrutinee's type — a
    /// *different* sum type, or a **primitive** (`Int`, `Bool`, `Text`, `Unit`)
    /// or other concrete type a variant can never inhabit — is a compile error
    /// (`T0012`, naming both types); it never contributes to coverage and its
    /// payload bindings are not instantiated from the wrong type. Only an
    /// already-erroneous (`Error`) or undetermined (`Infer`) scrutinee is left
    /// alone, so a single earlier mistake does not cascade.
    fn variant_matches_scrutinee(
        &mut self,
        owner: &str,
        scrutinee_ty: &Type,
        pattern_name: &str,
        span: Span,
    ) -> bool {
        match scrutinee_ty {
            // `owner` is by construction a sum type, so a scrutinee named the
            // same is that sum type — the pattern belongs here.
            Type::Named { name, .. } if name == owner => true,
            // Don't pile a second error onto an already-broken or undetermined
            // scrutinee.
            Type::Error | Type::Infer => true,
            // Any other concrete type — a different sum type, a primitive, a
            // capability, or a function — cannot hold this variant.
            other => {
                self.error(
                    "T0012",
                    span,
                    format!(
                        "pattern `{pattern_name}` belongs to type `{owner}`, but the matched value has type `{other}`"
                    ),
                );
                false
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

    /// Type-check a text built-in, or return `None` if `name` is not one.
    /// `Text` is a sequence of Unicode scalar values, so indices are char-based:
    /// `concat(Text, Text) -> Text`, `text_len(Text) -> Int`,
    /// `char_at(Text, Int) -> Text`, `substring(Text, Int, Int) -> Text`.
    fn check_text_builtin(
        &mut self,
        name: &str,
        arg_types: &[Type],
        args: &[Expr],
        span: Span,
    ) -> Option<Type> {
        let (params, ret): (Vec<Type>, Type) = match name {
            "concat" => (vec![Type::Text, Type::Text], Type::Text),
            "text_len" => (vec![Type::Text], Type::Int),
            "char_at" => (vec![Type::Text, Type::Int], Type::Text),
            "substring" => (vec![Type::Text, Type::Int, Type::Int], Type::Text),
            "char_code" => (vec![Type::Text], Type::Int),
            "code_char" => (vec![Type::Int], Type::Text),
            _ => return None,
        };
        if arg_types.len() != params.len() {
            self.error(
                "T0004",
                span,
                format!(
                    "`{name}` expects {} argument(s), found {}",
                    params.len(),
                    arg_types.len()
                ),
            );
            return Some(ret);
        }
        for (i, (expected, actual)) in params.iter().zip(arg_types).enumerate() {
            if !actual.compatible(expected) {
                self.error(
                    "T0001",
                    args[i].span(),
                    format!(
                        "argument {} to `{name}`: expected `{expected}`, found `{actual}`",
                        i + 1
                    ),
                );
            }
        }
        Some(ret)
    }

    /// Type-check a file-I/O built-in, or return `None` if `name` is not one.
    /// These are the first **effectful** built-ins: they take a capability of the
    /// matching authority (checked here) — `read_file(Cap<Read>, Text) -> Text`,
    /// `write_file(Cap<Write>, Text, Text) -> Unit`. Their effect (`Read` /
    /// `Write`) is enforced by the honesty and authority passes.
    fn check_io_builtin(
        &mut self,
        name: &str,
        arg_types: &[Type],
        args: &[Expr],
        span: Span,
    ) -> Option<Type> {
        let cap = |authority: &str| Type::Named {
            name: CAP.to_string(),
            args: vec![Type::Named {
                name: authority.to_string(),
                args: Vec::new(),
            }],
        };
        let (params, ret): (Vec<Type>, Type) = match name {
            "read_file" => (vec![cap("Read"), Type::Text], Type::Text),
            "write_file" => (vec![cap("Write"), Type::Text, Type::Text], Type::Unit),
            _ => return None,
        };
        if arg_types.len() != params.len() {
            self.error(
                "T0004",
                span,
                format!(
                    "`{name}` expects {} argument(s), found {}",
                    params.len(),
                    arg_types.len()
                ),
            );
            return Some(ret);
        }
        for (i, (expected, actual)) in params.iter().zip(arg_types).enumerate() {
            if !actual.compatible(expected) {
                self.error(
                    "T0001",
                    args[i].span(),
                    format!(
                        "argument {} to `{name}`: expected `{expected}`, found `{actual}`",
                        i + 1
                    ),
                );
            }
        }
        Some(ret)
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
                // Equality is defined only for types with structural value
                // semantics (`Int`, `Bool`, `Text`, sum types). Functions,
                // capabilities, and `Unit` have no spec'd equality and the
                // engines disagree (interp errors, native compares pointers /
                // strings / treats `Unit` as equal), so refuse them statically
                // (T0017) before that divergence can happen.
                let offender = uncomparable_kind(lt)
                    .map(|k| (lt, k))
                    .or_else(|| uncomparable_kind(rt).map(|k| (rt, k)));
                if let Some((ty, kind)) = offender {
                    self.error(
                        "T0017",
                        span,
                        format!(
                            "cannot compare {kind}: `==`/`!=` has no defined semantics for `{ty}`"
                        ),
                    );
                } else if !lt.compatible(rt) {
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

    /// Check a call's arguments against a parameter list: arity, then each
    /// argument's type. Shared by direct calls and higher-order calls.
    fn check_arg_types(
        &mut self,
        name: &str,
        params: &[Type],
        arg_types: &[Type],
        args: &[Expr],
        span: Span,
    ) {
        if arg_types.len() != params.len() {
            self.error(
                "T0004",
                span,
                format!(
                    "`{name}` expects {} argument(s), found {}",
                    params.len(),
                    arg_types.len()
                ),
            );
        }
        for (i, (param_ty, arg_ty)) in params.iter().zip(arg_types).enumerate() {
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

        // A **higher-order call**: the callee names a local or parameter that
        // holds a function value. (A global function is not in scope as a local,
        // so this only fires for function values; a local shadows a global.)
        if let Some(ty) = self.lookup(name) {
            return match ty {
                Type::Fn { params, ret } => {
                    self.check_arg_types(name, &params, &arg_types, args, span);
                    *ret
                }
                other => {
                    self.error(
                        "T0003",
                        span,
                        format!("`{name}` is not a function (it has type `{other}`) and cannot be called"),
                    );
                    Type::Error
                }
            };
        }

        // `grant<A>(cap)` narrows a capability to authority `A`. It is valid only
        // from a broader capability — the root capability `Cap<Root>` — so
        // authority can only ever be shed, never amplified.
        if name == GRANT {
            return self.check_grant(type_args, &arg_types, span);
        }

        // `sanitize(x)` removes taint: `Tainted<T> -> T`. Sanitizing a value that
        // is not tainted is a no-op on its type.
        // `sanitize(x, is_valid)` is the taint boundary: it applies the (pure)
        // validator `is_valid: fn(T) -> Bool` to the value inside `x:
        // Tainted<T>` and returns `Some(x)` if it passes, else `None` — a real,
        // rule-driven certification, not a no-op.
        if name == SANITIZE {
            if arg_types.len() != 2 {
                self.error(
                    "T0004",
                    span,
                    format!(
                        "`sanitize` expects 2 arguments (a `Tainted<T>` and a validator `fn(T) -> Bool`), found {}",
                        arg_types.len()
                    ),
                );
                return Type::Error;
            }
            // The trusted payload type: strip `Tainted<..>` if present.
            let inner = match &arg_types[0] {
                Type::Named { name, args } if name == TAINTED && args.len() == 1 => args[0].clone(),
                other => other.clone(),
            };
            let want_validator = Type::Fn {
                params: vec![inner.clone()],
                ret: Box::new(Type::Bool),
            };
            if !arg_types[1].compatible(&want_validator) {
                self.error(
                    "T0001",
                    args[1].span(),
                    format!(
                        "argument 2 to `sanitize`: expected a validator `{want_validator}`, found `{}`",
                        arg_types[1]
                    ),
                );
            }
            return Type::Named {
                name: "Option".to_string(),
                args: vec![inner],
            };
        }

        // Text built-ins (capability-free, like `print`): shadowable by a user
        // function of the same name.
        if !self.funcs.contains_key(name.as_str()) {
            if let Some(ty) = self.check_text_builtin(name, &arg_types, args, span) {
                return ty;
            }
            if let Some(ty) = self.check_io_builtin(name, &arg_types, args, span) {
                return ty;
            }
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

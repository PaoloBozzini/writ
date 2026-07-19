//! The taint pass: untrusted data cannot reach a sink without a `sanitize`
//! boundary.
//!
//! Untrusted data has type `Tainted<T>`. A value is **tainted** if it comes from
//! a `Tainted<T>` parameter (or a call to a function returning `Tainted<T>`), and
//! it stays tainted as it flows through `let` bindings — until `sanitize(x)`
//! strips it. A **sink** is a function that declares a dangerous effect
//! (`uses { Query }` or `uses { Shell }`). Passing a tainted value to a sink
//! without sanitizing it first is rejected (`E0401`).
//!
//! This pass is self-contained and imports no other checker. (The type system
//! independently keeps `Tainted<T>` distinct from `T`; this pass adds the
//! precise "reaches a sink" diagnostic and the `sanitize` boundary.)

use std::collections::{HashMap, HashSet};

use writ_ast::{Block, Diagnostic, Expr, Item, Module, Pattern, Stmt, TypeExpr};

/// The type-head marking untrusted data.
const TAINTED: &str = "Tainted";
/// The built-in that removes taint: `sanitize(Tainted<T>) -> T`.
const SANITIZE: &str = "sanitize";
/// Effects whose declaration marks a function as a sink.
const SINK_EFFECTS: [&str; 2] = ["Query", "Shell"];
/// The type-head of a function value: `fn(P, ...) -> R`.
const FN: &str = "fn";

fn is_tainted_type(ty: &TypeExpr) -> bool {
    ty.name == TAINTED
}

/// Whether `ty` is a function type whose **return** type is `Tainted<..>`. For
/// `fn(P, ...) -> R` the syntactic args are the parameter types followed by the
/// return type, so the return type is the last argument. Calling such a value
/// yields tainted data even though the callee is a local/parameter the
/// name-keyed `returns_tainted` table cannot see.
fn fn_type_returns_tainted(ty: &TypeExpr) -> bool {
    ty.name == FN && ty.args.last().is_some_and(is_tainted_type)
}

/// Check that no tainted value reaches a sink unsanitized. Empty result means
/// every sink argument is trusted.
#[must_use]
pub fn check_taint(module: &Module) -> Vec<Diagnostic> {
    let mut returns_tainted: HashMap<&str, bool> = HashMap::new();
    let mut is_sink: HashMap<&str, bool> = HashMap::new();
    let mut ctors: HashSet<&str> = HashSet::new();
    for item in &module.items {
        match item {
            Item::Function(f) => {
                returns_tainted.insert(
                    f.signature.name.as_str(),
                    f.signature
                        .return_type
                        .as_ref()
                        .is_some_and(is_tainted_type),
                );
                let sink = f
                    .signature
                    .effects
                    .effects
                    .iter()
                    .any(|e| SINK_EFFECTS.contains(&e.name.as_str()));
                is_sink.insert(f.signature.name.as_str(), sink);
            }
            Item::Type(decl) => {
                for variant in &decl.variants {
                    ctors.insert(variant.name.as_str());
                }
            }
        }
    }

    let mut checker = TaintChecker {
        returns_tainted,
        is_sink,
        ctors,
        diagnostics: Vec::new(),
        scopes: Vec::new(),
    };
    for item in &module.items {
        let Item::Function(f) = item else {
            continue;
        };
        // Seed the outermost scope: parameters typed `Tainted<T>` are tainted,
        // and parameters typed `fn(..) -> Tainted<..>` taint whatever they
        // return when called.
        let mut scope = Scope::default();
        for param in &f.signature.params {
            if is_tainted_type(&param.ty) {
                scope.tainted.insert(param.name.clone());
            }
            if fn_type_returns_tainted(&param.ty) {
                scope.tainting_fns.insert(param.name.clone());
            }
        }
        checker.scopes = vec![scope];
        checker.check_block(&f.body);
    }
    checker.diagnostics
}

/// A lexical scope's taint facts: which names hold tainted values, and which
/// names hold function values whose call returns tainted data.
#[derive(Default)]
struct Scope {
    /// Names bound to tainted values.
    tainted: HashSet<String>,
    /// Names bound to `fn(..) -> Tainted<..>` values (calling them taints).
    tainting_fns: HashSet<String>,
}

struct TaintChecker<'m> {
    returns_tainted: HashMap<&'m str, bool>,
    is_sink: HashMap<&'m str, bool>,
    /// Sum-type constructor names — a constructor preserves its arguments' taint
    /// inside the value it builds.
    ctors: HashSet<&'m str>,
    diagnostics: Vec<Diagnostic>,
    /// Taint facts per lexical scope, innermost scope last.
    scopes: Vec<Scope>,
}

impl TaintChecker<'_> {
    fn is_tainted_name(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.tainted.contains(name))
    }

    fn mark_tainted(&mut self, name: String) {
        self.scopes
            .last_mut()
            .expect("a scope")
            .tainted
            .insert(name);
    }

    /// Whether calling the value named `name` yields tainted data: a top-level
    /// function declared to return `Tainted<..>`, or a local/parameter bound to
    /// such a function value.
    fn call_of_name_is_tainted(&self, name: &str) -> bool {
        self.returns_tainted.get(name).copied().unwrap_or(false)
            || self.scopes.iter().any(|s| s.tainting_fns.contains(name))
    }

    fn mark_tainting_fn(&mut self, name: String) {
        self.scopes
            .last_mut()
            .expect("a scope")
            .tainting_fns
            .insert(name);
    }

    /// Whether an expression evaluates to a tainted value.
    ///
    /// Taint is tracked **structurally**, not syntactically: wrapping a tainted
    /// value in any compound expression keeps it tainted, so a `match`/`if`/
    /// operator/constructor cannot launder it past a sink. Taint is cleared only
    /// by a `sanitize(..)` call or by a call whose result type is trusted.
    fn is_tainted(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Identifier { name, .. } => self.is_tainted_name(name),
            Expr::Call { callee, args, .. } => {
                let Expr::Identifier { name, .. } = callee.as_ref() else {
                    // A cross-module (`Expr::Member`) callee's taint is decided
                    // once linking makes it a qualified identifier; here, treat
                    // an unresolved callee as untainted.
                    return false;
                };
                if name == SANITIZE {
                    return false;
                }
                // A constructor keeps its payload's taint inside the value it
                // builds, so `Some(tainted)` is tainted.
                if self.ctors.contains(name.as_str()) {
                    return args.iter().any(|a| self.is_tainted(a));
                }
                // Any other call taints only if its result type is `Tainted<..>`
                // — whether the callee is a top-level function or a `fn`-typed
                // local/parameter. A call's *arguments* never taint the result:
                // a function returning `Text` genuinely de-taints.
                self.call_of_name_is_tainted(name)
            }
            // A `match` is tainted if the scrutinee is (its bindings could carry
            // that taint out) or if any arm body is. This over-approximates in
            // the safe direction — the security guarantee never under-taints.
            Expr::Match {
                scrutinee, arms, ..
            } => self.is_tainted(scrutinee) || arms.iter().any(|a| self.is_tainted(&a.body)),
            // Operators propagate taint from either operand.
            Expr::Binary { left, right, .. } => self.is_tainted(left) || self.is_tainted(right),
            Expr::Unary { operand, .. } => self.is_tainted(operand),
            Expr::Member { base, .. } => self.is_tainted(base),
            Expr::Literal(_) => false,
        }
    }

    fn check_block(&mut self, block: &Block) {
        self.scopes.push(Scope::default());
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
        self.scopes.pop();
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, value, .. } => {
                self.check_sinks(value);
                if self.is_tainted(value) {
                    self.mark_tainted(name.clone());
                }
                // A `let` that binds an existing tainting function value (an
                // alias, or a top-level tainting function used as a value)
                // carries the "calling this taints" fact to the new name.
                if let Expr::Identifier { name: rhs, .. } = value {
                    if self.call_of_name_is_tainted(rhs) {
                        self.mark_tainting_fn(name.clone());
                    }
                }
            }
            Stmt::Expr(e) => self.check_sinks(e),
            Stmt::Return { value: Some(e), .. } => self.check_sinks(e),
            Stmt::Return { value: None, .. } => {}
            Stmt::Check { predicate, .. } => self.check_sinks(predicate),
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.check_sinks(cond);
                self.check_block(then_block);
                if let Some(else_block) = else_block {
                    self.check_block(else_block);
                }
            }
        }
    }

    /// Find every call in `expr`; at a sink call, reject any tainted argument.
    fn check_sinks(&mut self, expr: &Expr) {
        match expr {
            Expr::Call {
                callee, args, span, ..
            } => {
                if let Expr::Identifier { name, .. } = callee.as_ref() {
                    if self.is_sink.get(name.as_str()).copied().unwrap_or(false) {
                        for arg in args {
                            if self.is_tainted(arg) {
                                self.diagnostics.push(Diagnostic::error(
                                    "E0401",
                                    *span,
                                    format!(
                                        "a tainted value reaches the sink `{name}` here without a `sanitize` boundary"
                                    ),
                                ));
                            }
                        }
                    }
                }
                self.check_sinks(callee);
                for arg in args {
                    self.check_sinks(arg);
                }
            }
            Expr::Unary { operand, .. } => self.check_sinks(operand),
            Expr::Binary { left, right, .. } => {
                self.check_sinks(left);
                self.check_sinks(right);
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.check_sinks(scrutinee);
                // When the scrutinee is tainted, every variable an arm's pattern
                // binds extracts data that could be tainted, so bind them
                // tainted for the arm body. This is what stops a
                // `Some(tainted)` / `match { Some(x) => sink(x) }` unwrap from
                // laundering the taint.
                let scrutinee_tainted = self.is_tainted(scrutinee);
                for arm in arms {
                    self.scopes.push(Scope::default());
                    if scrutinee_tainted {
                        self.bind_pattern_tainted(&arm.pattern);
                    }
                    self.check_sinks(&arm.body);
                    self.scopes.pop();
                }
            }
            Expr::Member { base, .. } => self.check_sinks(base),
            Expr::Literal(_) | Expr::Identifier { .. } => {}
        }
    }

    /// Mark every variable a pattern binds as tainted (in the current scope).
    /// A nullary-variant name binds nothing; a `Variant(sub, ..)` recurses.
    /// Over-approximating — marking *all* of a constructor's bindings rather
    /// than only the tainted fields — keeps the guarantee sound.
    fn bind_pattern_tainted(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Wildcard { .. } => {}
            Pattern::Ident { name, .. } => {
                // A constructor name (a nullary variant) binds nothing; only a
                // real binding introduces a name.
                if !self.ctors.contains(name.as_str()) {
                    self.mark_tainted(name.clone());
                }
            }
            Pattern::Variant { args, .. } => {
                for sub in args {
                    self.bind_pattern_tainted(sub);
                }
            }
        }
    }
}

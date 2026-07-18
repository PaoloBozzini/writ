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

use writ_ast::{Block, Diagnostic, Expr, Item, Module, Stmt, TypeExpr};

/// The type-head marking untrusted data.
const TAINTED: &str = "Tainted";
/// The built-in that removes taint: `sanitize(Tainted<T>) -> T`.
const SANITIZE: &str = "sanitize";
/// Effects whose declaration marks a function as a sink.
const SINK_EFFECTS: [&str; 2] = ["Query", "Shell"];

fn is_tainted_type(ty: &TypeExpr) -> bool {
    ty.name == TAINTED
}

/// Check that no tainted value reaches a sink unsanitized. Empty result means
/// every sink argument is trusted.
#[must_use]
pub fn check_taint(module: &Module) -> Vec<Diagnostic> {
    let mut returns_tainted: HashMap<&str, bool> = HashMap::new();
    let mut is_sink: HashMap<&str, bool> = HashMap::new();
    for item in &module.items {
        let Item::Function(f) = item else {
            continue;
        };
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

    let mut checker = TaintChecker {
        returns_tainted,
        is_sink,
        diagnostics: Vec::new(),
        scopes: Vec::new(),
    };
    for item in &module.items {
        let Item::Function(f) = item else {
            continue;
        };
        checker.scopes = vec![f
            .signature
            .params
            .iter()
            .filter(|p| is_tainted_type(&p.ty))
            .map(|p| p.name.clone())
            .collect()];
        checker.check_block(&f.body);
    }
    checker.diagnostics
}

struct TaintChecker<'m> {
    returns_tainted: HashMap<&'m str, bool>,
    is_sink: HashMap<&'m str, bool>,
    diagnostics: Vec<Diagnostic>,
    /// Names currently bound to tainted values, innermost scope last.
    scopes: Vec<HashSet<String>>,
}

impl TaintChecker<'_> {
    fn is_tainted_name(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(name))
    }

    fn mark_tainted(&mut self, name: String) {
        self.scopes.last_mut().expect("a scope").insert(name);
    }

    /// Whether an expression evaluates to a tainted value.
    ///
    /// Taint is tracked **structurally**, not syntactically: wrapping a tainted
    /// value in any compound expression keeps it tainted, so a `match`/`if`/
    /// operator cannot launder it past a sink. Only two things clear taint — a
    /// `sanitize(..)` call and a call to a function not declared to return
    /// `Tainted<T>` (its result type, not its arguments, decides).
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
                // The text-returning built-ins **propagate** taint from their
                // textual argument(s): slicing or joining untrusted data keeps it
                // untrusted. `text_len` / `char_code` (→ `Int`) and `code_char`
                // (fresh text) do not. Any other function taints only if it is
                // declared to return `Tainted<T>`.
                match name.as_str() {
                    "char_at" | "substring" => args.first().is_some_and(|a| self.is_tainted(a)),
                    "concat" => args.iter().any(|a| self.is_tainted(a)),
                    "text_len" | "char_code" | "code_char" => false,
                    _ => self
                        .returns_tainted
                        .get(name.as_str())
                        .copied()
                        .unwrap_or(false),
                }
            }
            // A `match` result is tainted if any arm it could pick is tainted.
            Expr::Match { arms, .. } => arms.iter().any(|a| self.is_tainted(&a.body)),
            // Operators propagate taint from either operand.
            Expr::Binary { left, right, .. } => self.is_tainted(left) || self.is_tainted(right),
            Expr::Unary { operand, .. } => self.is_tainted(operand),
            Expr::Member { base, .. } => self.is_tainted(base),
            Expr::Literal(_) => false,
        }
    }

    fn check_block(&mut self, block: &Block) {
        self.scopes.push(HashSet::new());
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
                for arm in arms {
                    self.check_sinks(&arm.body);
                }
            }
            Expr::Member { base, .. } => self.check_sinks(base),
            Expr::Literal(_) | Expr::Identifier { .. } => {}
        }
    }
}

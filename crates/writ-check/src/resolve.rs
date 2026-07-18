//! The module resolver: qualified names and visibility.
//!
//! A program is a collection of named modules (built from files by the driver,
//! #83). This pass checks, for every module:
//!
//! - each `import` names a module that exists (`R0001`);
//! - each `module.member` access has an imported-module base (`R0002`), names an
//!   item that exists (`R0003`), and that item is `export`ed (`R0004`) — using a
//!   private item across a module boundary is refused, the module-level twin of
//!   "unreachable by default"; and
//! - the import graph has no cycles (`R0005`).
//!
//! This pass is self-contained and imports no other checker.

use std::collections::{BTreeMap, BTreeSet};

use writ_ast::{Block, Diagnostic, Expr, Item, Module, Span, Stmt};

/// Resolve qualified names and enforce visibility across a set of named modules.
/// Returns diagnostics in deterministic (module-sorted) order.
#[must_use]
pub fn check_resolution(modules: &BTreeMap<String, Module>) -> Vec<Diagnostic> {
    let exported: BTreeMap<&str, BTreeSet<&str>> = modules
        .iter()
        .map(|(n, m)| (n.as_str(), item_names(m, true)))
        .collect();
    let all: BTreeMap<&str, BTreeSet<&str>> = modules
        .iter()
        .map(|(n, m)| (n.as_str(), item_names(m, false)))
        .collect();

    let mut diagnostics = Vec::new();
    for (mod_name, module) in modules {
        let imported: BTreeSet<&str> = module.imports.iter().map(|i| i.name.as_str()).collect();

        // Each import must name a module that exists, and must not close a cycle.
        for import in &module.imports {
            if !modules.contains_key(&import.name) {
                diagnostics.push(Diagnostic::error(
                    "R0001",
                    import.span,
                    format!("unknown module `{}`", import.name),
                ));
            } else if reaches(&import.name, mod_name, modules) {
                diagnostics.push(Diagnostic::error(
                    "R0005",
                    import.span,
                    format!(
                        "import cycle: `{mod_name}` and `{}` import each other",
                        import.name
                    ),
                ));
            }
        }

        // Each `module.member` access must resolve to an exported item.
        let mut members = Vec::new();
        collect_members(module, &mut members);
        for (base, name, span) in members {
            if !imported.contains(base) {
                diagnostics.push(Diagnostic::error(
                    "R0002",
                    span,
                    format!("`{base}` is not an imported module"),
                ));
                continue;
            }
            let Some(items) = all.get(base) else {
                continue; // unknown module already reported as R0001
            };
            if !items.contains(name) {
                diagnostics.push(Diagnostic::error(
                    "R0003",
                    span,
                    format!("module `{base}` has no item `{name}`"),
                ));
            } else if !exported.get(base).is_some_and(|e| e.contains(name)) {
                diagnostics.push(Diagnostic::error(
                    "R0004",
                    span,
                    format!("`{name}` is private to module `{base}` — mark it `export` to use it"),
                ));
            }
        }
    }
    diagnostics
}

/// The names of a module's items — all of them, or only the `export`ed ones.
fn item_names(module: &Module, only_exported: bool) -> BTreeSet<&str> {
    module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(f) if f.exported || !only_exported => Some(f.signature.name.as_str()),
            Item::Type(t) if t.exported || !only_exported => Some(t.name.as_str()),
            _ => None,
        })
        .collect()
}

/// Whether module `start` reaches `target` by following imports (used for cycle
/// detection: an edge `m -> i` is cyclic iff `i` reaches `m`).
fn reaches(start: &str, target: &str, modules: &BTreeMap<String, Module>) -> bool {
    let mut stack = vec![start];
    let mut seen = BTreeSet::new();
    while let Some(cur) = stack.pop() {
        if cur == target {
            return true;
        }
        if !seen.insert(cur) {
            continue;
        }
        if let Some(m) = modules.get(cur) {
            for import in &m.imports {
                stack.push(import.name.as_str());
            }
        }
    }
    false
}

/// Collect every `base.member` access (with an identifier base) in a module.
fn collect_members<'a>(module: &'a Module, out: &mut Vec<(&'a str, &'a str, Span)>) {
    for item in &module.items {
        if let Item::Function(f) = item {
            for clause in f.signature.requires.iter().chain(&f.signature.ensures) {
                collect_members_expr(&clause.predicate, out);
            }
            collect_members_block(&f.body, out);
        }
    }
}

fn collect_members_block<'a>(block: &'a Block, out: &mut Vec<(&'a str, &'a str, Span)>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { value, .. } => collect_members_expr(value, out),
            Stmt::Expr(e) => collect_members_expr(e, out),
            Stmt::Return { value: Some(e), .. } => collect_members_expr(e, out),
            Stmt::Return { value: None, .. } => {}
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                collect_members_expr(cond, out);
                collect_members_block(then_block, out);
                if let Some(else_block) = else_block {
                    collect_members_block(else_block, out);
                }
            }
        }
    }
}

fn collect_members_expr<'a>(expr: &'a Expr, out: &mut Vec<(&'a str, &'a str, Span)>) {
    match expr {
        Expr::Member { base, name, span } => {
            if let Expr::Identifier {
                name: base_name, ..
            } = base.as_ref()
            {
                out.push((base_name.as_str(), name.as_str(), *span));
            }
            collect_members_expr(base, out);
        }
        Expr::Unary { operand, .. } => collect_members_expr(operand, out),
        Expr::Binary { left, right, .. } => {
            collect_members_expr(left, out);
            collect_members_expr(right, out);
        }
        Expr::Call { callee, args, .. } => {
            collect_members_expr(callee, out);
            for arg in args {
                collect_members_expr(arg, out);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_members_expr(scrutinee, out);
            for arm in arms {
                collect_members_expr(&arm.body, out);
            }
        }
        Expr::Literal(_) | Expr::Identifier { .. } => {}
    }
}

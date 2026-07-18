//! `writ` driver: load a program from files, check it, and run it.
//!
//! A program is a root `.writ` file plus the sibling files its `import`s name.
//! This crate is the thin wiring layer — all language logic lives in the
//! pipeline crates.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use writ_ast::{Block, Diagnostic, Expr, Function, Item, Module, Span, Stmt};
use writ_interp::{Interpreter, RuntimeError, Value};

/// A loaded program: its modules keyed by name, and the root module's name.
pub struct Program {
    pub modules: BTreeMap<String, Module>,
    pub root: String,
}

/// A module name is the file stem (`math.writ` → `math`).
fn module_name(path: &Path) -> String {
    path.file_stem()
        .map_or_else(|| "main".to_string(), |s| s.to_string_lossy().into_owned())
}

/// Load the root file and, transitively, the sibling files its imports name.
/// Load/parse diagnostics are returned alongside the (possibly partial) program.
#[must_use]
pub fn load_program(root_path: &Path) -> (Program, Vec<Diagnostic>) {
    let dir = root_path
        .parent()
        .map_or_else(|| Path::new(".").to_path_buf(), Path::to_path_buf);
    let root = module_name(root_path);
    let mut modules = BTreeMap::new();
    let mut diagnostics = Vec::new();

    let mut queue = vec![(root.clone(), root_path.to_path_buf())];
    while let Some((name, path)) = queue.pop() {
        if modules.contains_key(&name) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            diagnostics.push(Diagnostic::error(
                "D0001",
                Span::new(0, 0),
                format!(
                    "cannot read module `{name}` (expected file `{}`)",
                    path.display()
                ),
            ));
            continue;
        };
        let parsed = writ_parser::parse(&src);
        diagnostics.extend(parsed.diagnostics);
        for import in &parsed.module.imports {
            if !modules.contains_key(&import.name) {
                queue.push((
                    import.name.clone(),
                    dir.join(format!("{}.writ", import.name)),
                ));
            }
        }
        modules.insert(name, parsed.module);
    }

    (Program { modules, root }, diagnostics)
}

/// The independent check passes, in run order. Each is its own module in
/// `writ-check` with no cross-pass imports, so any subset can run alone.
pub const PASSES: &[&str] = &[
    "resolution",
    "types",
    "effects",
    "authority",
    "capabilities",
    "taint",
];

/// Run every static check over a program.
#[must_use]
pub fn check(program: &Program) -> Vec<Diagnostic> {
    check_passes(program, &[])
}

/// Run only the named passes (an empty selection runs them all), demonstrating
/// that the passes are independent — each can run without the others. Returns
/// diagnostics in a stable order.
#[must_use]
pub fn check_passes(program: &Program, passes: &[String]) -> Vec<Diagnostic> {
    let enabled = |name: &str| passes.is_empty() || passes.iter().any(|p| p == name);
    let mut diagnostics = Vec::new();
    // The resolver is the one cross-module pass; the rest are module-local.
    if enabled("resolution") {
        diagnostics.extend(writ_check::check_resolution(&program.modules));
    }
    for module in program.modules.values() {
        if enabled("types") {
            diagnostics.extend(writ_check::check_types(module));
        }
        if enabled("effects") {
            diagnostics.extend(writ_check::check_effects(module));
        }
        if enabled("authority") {
            diagnostics.extend(writ_check::check_authority(module));
        }
        if enabled("capabilities") {
            diagnostics.extend(writ_check::check_capabilities(module));
        }
        if enabled("taint") {
            diagnostics.extend(writ_check::check_taint(module));
        }
    }
    diagnostics
}

/// Run a checked program's `main` via the interpreter, returning the lines it
/// printed. Multi-module programs are linked into one module (functions
/// qualified by module) first, and `main` is handed a root capability for each
/// of its capability parameters.
///
/// # Errors
/// Returns a [`RuntimeError`] if there is no `main`, or execution fails.
pub fn run(program: &Program) -> Result<Vec<String>, RuntimeError> {
    let linked = link(program);
    let interp = Interpreter::new(&linked)?;
    let main = linked
        .items
        .iter()
        .find_map(|it| match it {
            Item::Function(f) if f.signature.name == "main" => Some(f),
            _ => None,
        })
        .ok_or_else(|| RuntimeError::new(Span::new(0, 0), "no `main` function"))?;
    let args = main
        .signature
        .params
        .iter()
        .map(|p| {
            if p.ty.name == "Cap" {
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
    interp.call("main", args)?;
    Ok(interp.output())
}

/// Flatten a program into a single module: each imported module's functions are
/// renamed `module.name`, and calls are rewritten to match. The root module's
/// names are left unqualified so `main` stays `main`.
fn link(program: &Program) -> Module {
    let mut items = Vec::new();
    for (mod_name, module) in &program.modules {
        let is_root = *mod_name == program.root;
        let local_fns: BTreeSet<&str> = module
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Function(f) => Some(f.signature.name.as_str()),
                Item::Type(_) => None,
            })
            .collect();
        for item in &module.items {
            match item {
                Item::Function(f) => {
                    let mut nf: Function = f.clone();
                    nf.signature.name = qualify(mod_name, &f.signature.name, is_root);
                    nf.body = rewrite_block(&f.body, mod_name, &local_fns, is_root);
                    items.push(Item::Function(nf));
                }
                // Sum-type constructors are global names in the interpreter.
                Item::Type(t) => items.push(Item::Type(t.clone())),
            }
        }
    }
    Module {
        imports: Vec::new(),
        items,
    }
}

fn qualify(module: &str, name: &str, is_root: bool) -> String {
    if is_root {
        name.to_string()
    } else {
        format!("{module}.{name}")
    }
}

fn rewrite_block(block: &Block, module: &str, local_fns: &BTreeSet<&str>, is_root: bool) -> Block {
    Block {
        stmts: block
            .stmts
            .iter()
            .map(|s| rewrite_stmt(s, module, local_fns, is_root))
            .collect(),
        span: block.span,
    }
}

fn rewrite_stmt(stmt: &Stmt, module: &str, local_fns: &BTreeSet<&str>, is_root: bool) -> Stmt {
    let re = |e: &Expr| rewrite_expr(e, module, local_fns, is_root);
    match stmt {
        Stmt::Let {
            name,
            mutable,
            ty,
            value,
            span,
        } => Stmt::Let {
            name: name.clone(),
            mutable: *mutable,
            ty: ty.clone(),
            value: re(value),
            span: *span,
        },
        Stmt::Expr(e) => Stmt::Expr(re(e)),
        Stmt::Return { value, span } => Stmt::Return {
            value: value.as_ref().map(re),
            span: *span,
        },
        Stmt::If {
            cond,
            then_block,
            else_block,
            span,
        } => Stmt::If {
            cond: re(cond),
            then_block: rewrite_block(then_block, module, local_fns, is_root),
            else_block: else_block
                .as_ref()
                .map(|b| rewrite_block(b, module, local_fns, is_root)),
            span: *span,
        },
    }
}

fn rewrite_expr(expr: &Expr, module: &str, local_fns: &BTreeSet<&str>, is_root: bool) -> Expr {
    let re = |e: &Expr| rewrite_expr(e, module, local_fns, is_root);
    match expr {
        Expr::Call {
            callee,
            type_args,
            args,
            span,
        } => {
            let new_callee = match callee.as_ref() {
                // Cross-module call `m.f(..)` -> a function named `m.f`.
                Expr::Member {
                    base,
                    name,
                    span: mspan,
                } => {
                    if let Expr::Identifier {
                        name: base_name, ..
                    } = base.as_ref()
                    {
                        Expr::Identifier {
                            name: format!("{base_name}.{name}"),
                            span: *mspan,
                        }
                    } else {
                        re(callee)
                    }
                }
                // Intra-module call in a non-root module -> qualified name.
                Expr::Identifier { name, span: ispan }
                    if !is_root && local_fns.contains(name.as_str()) =>
                {
                    Expr::Identifier {
                        name: format!("{module}.{name}"),
                        span: *ispan,
                    }
                }
                other => re(other),
            };
            Expr::Call {
                callee: Box::new(new_callee),
                type_args: type_args.clone(),
                args: args.iter().map(&re).collect(),
                span: *span,
            }
        }
        Expr::Unary { op, operand, span } => Expr::Unary {
            op: *op,
            operand: Box::new(re(operand)),
            span: *span,
        },
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => Expr::Binary {
            op: *op,
            left: Box::new(re(left)),
            right: Box::new(re(right)),
            span: *span,
        },
        Expr::Match {
            scrutinee,
            arms,
            span,
        } => Expr::Match {
            scrutinee: Box::new(re(scrutinee)),
            arms: arms
                .iter()
                .map(|a| writ_ast::MatchArm {
                    pattern: a.pattern.clone(),
                    body: re(&a.body),
                    span: a.span,
                })
                .collect(),
            span: *span,
        },
        // A bare `m.f` reference (not in call position) becomes the linked name.
        Expr::Member { base, name, span } => {
            if let Expr::Identifier {
                name: base_name, ..
            } = base.as_ref()
            {
                Expr::Identifier {
                    name: format!("{base_name}.{name}"),
                    span: *span,
                }
            } else {
                Expr::Member {
                    base: Box::new(re(base)),
                    name: name.clone(),
                    span: *span,
                }
            }
        }
        Expr::Literal(_) | Expr::Identifier { .. } => expr.clone(),
    }
}

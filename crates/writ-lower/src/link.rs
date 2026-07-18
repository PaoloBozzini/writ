//! **Linking** — flatten a multi-module program into one module that both the
//! checkers and every back end consume.
//!
//! A program is a set of named modules. Linking renames each non-root module's
//! functions to `module.name` and rewrites every call site to match, so a
//! cross-module call `m.f(..)` becomes an ordinary call to a function named
//! `m.f`. The root module keeps unqualified names, so `main` stays `main`.
//!
//! Doing this **before** the checkers (not just before execution) is the whole
//! point: once cross-module calls are plain identifier calls in one module, the
//! module-local passes — types, effects, authority, taint — see them like any
//! other call, so authority and honesty apply across boundaries exactly as
//! spec §5 promises. It also means the artifact that is checked is the artifact
//! that runs (issue #103): no separate rewrite lives in the driver.

use std::collections::{BTreeMap, BTreeSet};

use writ_ast::{
    Block, Expr, Function, Item, MatchArm, Module, Span, Stmt, TypeDecl, TypeExpr, Variant,
};

/// Flatten `modules` into a single module. `root` is the entry module whose
/// names stay unqualified.
#[must_use]
pub fn link(modules: &BTreeMap<String, Module>, root: &str) -> Module {
    let mut items = Vec::new();
    for (mod_name, module) in modules {
        let is_root = mod_name == root;
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
                // Sum-type constructors are global names shared across modules.
                Item::Type(t) => items.push(Item::Type(t.clone())),
            }
        }
    }
    add_prelude(&mut items);

    Module {
        imports: Vec::new(),
        items,
    }
}

/// Prepend the **prelude** — the standard sum types that are always in scope
/// (`Option<T>`, `Result<T, E>`) — so a program can use `Some` / `None` / `Ok` /
/// `Err` without declaring or importing anything.
///
/// A prelude type is skipped when the program already declares a type of the
/// same name, or a variant (constructor) of the same name, so a user-declared
/// `Option` **shadows** the built-in and nothing is forced on a program that
/// wants its own.
fn add_prelude(items: &mut Vec<Item>) {
    let type_names: BTreeSet<&str> = items
        .iter()
        .filter_map(|i| match i {
            Item::Type(t) => Some(t.name.as_str()),
            Item::Function(_) => None,
        })
        .collect();
    let variant_names: BTreeSet<&str> = items
        .iter()
        .filter_map(|i| match i {
            Item::Type(t) => Some(t),
            Item::Function(_) => None,
        })
        .flat_map(|t| t.variants.iter().map(|v| v.name.as_str()))
        .collect();

    let mut prelude = Vec::new();
    for decl in prelude_types() {
        let collides = type_names.contains(decl.name.as_str())
            || decl
                .variants
                .iter()
                .any(|v| variant_names.contains(v.name.as_str()));
        if !collides {
            prelude.push(Item::Type(decl));
        }
    }
    // Prepend so the prelude precedes user items in source-ordered diagnostics.
    prelude.append(items);
    *items = prelude;
}

/// The prelude type declarations, built directly as AST (they are ordinary
/// generic sum types — the checker and back ends need no special knowledge of
/// them).
fn prelude_types() -> Vec<TypeDecl> {
    // `type Option<T> = Some(T) | None`
    let option = TypeDecl {
        exported: false,
        name: "Option".to_string(),
        generics: vec!["T".to_string()],
        variants: vec![variant("Some", &["T"]), variant("None", &[])],
        span: Span::new(0, 0),
    };
    // `type Result<T, E> = Ok(T) | Err(E)`
    let result = TypeDecl {
        exported: false,
        name: "Result".to_string(),
        generics: vec!["T".to_string(), "E".to_string()],
        variants: vec![variant("Ok", &["T"]), variant("Err", &["E"])],
        span: Span::new(0, 0),
    };
    vec![option, result]
}

fn variant(name: &str, fields: &[&str]) -> Variant {
    Variant {
        name: name.to_string(),
        fields: fields
            .iter()
            .map(|f| TypeExpr {
                name: (*f).to_string(),
                args: Vec::new(),
                span: Span::new(0, 0),
            })
            .collect(),
        span: Span::new(0, 0),
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
        Stmt::Check {
            predicate,
            blame,
            span,
        } => Stmt::Check {
            predicate: re(predicate),
            blame: *blame,
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
                .map(|a| MatchArm {
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

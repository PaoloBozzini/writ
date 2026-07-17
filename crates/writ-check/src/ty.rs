//! The type representation used by the type checker.
//!
//! This is the checker's *internal* view of types, resolved from the syntactic
//! `TypeExpr` in the AST. It is intentionally not stored back onto the AST: the
//! checker computes types transiently and reports diagnostics, which keeps the
//! pass-annotation strategy (typed AST vs side tables) an open decision.

use std::fmt;

use writ_ast::TypeExpr;

/// A resolved type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Bool,
    Text,
    /// The type of a statement or a function with no return value.
    Unit,
    /// Any nominal type the checker does not have built-in rules for, kept
    /// opaque so later passes (e.g. the capability authority checker) can give
    /// it meaning. `Cap<Write>` resolves to `Named { "Cap", [Named "Write"] }`.
    Named {
        name: String,
        args: Vec<Type>,
    },
    /// A placeholder for an already-reported error, so a single mistake does not
    /// cascade into a flood of follow-on diagnostics.
    Error,
}

impl Type {
    /// Resolve a syntactic type into a checker type.
    #[must_use]
    pub fn resolve(t: &TypeExpr) -> Type {
        match t.name.as_str() {
            "Int" => Type::Int,
            "Bool" => Type::Bool,
            "Text" => Type::Text,
            "Unit" => Type::Unit,
            _ => Type::Named {
                name: t.name.clone(),
                args: t.args.iter().map(Type::resolve).collect(),
            },
        }
    }

    /// Whether two types are compatible. There are **no implicit coercions**, so
    /// this is exact equality — except that [`Type::Error`] is compatible with
    /// anything, to avoid cascading diagnostics after a first error.
    #[must_use]
    pub fn compatible(&self, other: &Type) -> bool {
        matches!(self, Type::Error) || matches!(other, Type::Error) || self == other
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Int => write!(f, "Int"),
            Type::Bool => write!(f, "Bool"),
            Type::Text => write!(f, "Text"),
            Type::Unit => write!(f, "Unit"),
            Type::Error => write!(f, "<error>"),
            Type::Named { name, args } => {
                write!(f, "{name}")?;
                if let Some((first, rest)) = args.split_first() {
                    write!(f, "<{first}")?;
                    for a in rest {
                        write!(f, ", {a}")?;
                    }
                    write!(f, ">")?;
                }
                Ok(())
            }
        }
    }
}

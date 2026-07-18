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
    /// An as-yet-undetermined generic argument — e.g. the `T` in `None`'s
    /// `Option<T>`, which no field fixes. It unifies with any type, so a
    /// nullary constructor stays polymorphic without a full inference engine.
    Infer,
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
    /// this is structural equality — with two wildcards that match anything:
    /// [`Type::Error`] (to avoid cascading diagnostics after a first error) and
    /// [`Type::Infer`] (an undetermined generic argument). Named types match
    /// head-and-arity and recurse into their arguments, so `Option<Int>` is
    /// incompatible with `Option<Text>` but compatible with `Option<_>`.
    #[must_use]
    pub fn compatible(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Error | Type::Infer, _) | (_, Type::Error | Type::Infer) => true,
            (Type::Named { name: n1, args: a1 }, Type::Named { name: n2, args: a2 }) => {
                n1 == n2 && a1.len() == a2.len() && a1.iter().zip(a2).all(|(x, y)| x.compatible(y))
            }
            _ => self == other,
        }
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
            Type::Infer => write!(f, "_"),
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

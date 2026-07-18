//! The abstract syntax tree.
//!
//! This is a *plain, untyped* AST: nodes carry spans but no type, effect, or
//! contract-verification results. Where those pass results eventually live —
//! baked into a typed AST or held in side tables — is deliberately left open
//! (see the pass-annotation design issue) so this module can serve both the
//! interpreter and the future compiler unchanged.
//!
//! Signatures are the load-bearing surface of Writ, so [`Signature`] has
//! first-class room for the effect set (`uses {...}`) and the contract clauses
//! (`requires` / `ensures`) rather than treating them as afterthoughts.

use crate::span::Span;

/// A literal value written directly in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiteralKind {
    /// An integer literal, e.g. `42`.
    Int(i64),
    /// A boolean literal, `true` or `false`.
    Bool(bool),
    /// A text literal, e.g. `"hello"`.
    Text(String),
}

/// A literal expression together with its source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Literal {
    pub kind: LiteralKind,
    pub span: Span,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Arithmetic negation, `-x`.
    Neg,
    /// Logical negation, `!x`.
    Not,
}

/// Binary operators. Precedence is resolved by the parser and reflected in the
/// tree shape, not stored on the node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// An expression node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A literal value.
    Literal(Literal),
    /// A reference to a binding or function by name.
    Identifier { name: String, span: Span },
    /// A unary operation, e.g. `-x` or `!ok`.
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
        span: Span,
    },
    /// A binary operation, e.g. `a + b`. Grouping is expressed by the tree, so
    /// there is no dedicated parenthesized node.
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    /// A function call, e.g. `f(a, b)`. Sum-type constructors reuse this form:
    /// `Some(x)` is a call, `None` is an `Identifier`.
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    /// A `match` expression over a scrutinee.
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
}

impl Expr {
    /// The source span covering this expression.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Expr::Literal(lit) => lit.span,
            Expr::Identifier { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Call { span, .. }
            | Expr::Match { span, .. } => *span,
        }
    }
}

/// One arm of a `match`: a pattern and the expression it evaluates to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub span: Span,
}

/// A match pattern.
///
/// The parser does not know which names are sum-type variants, so a bare
/// identifier is left as [`Pattern::Ident`] — a later pass classifies it as a
/// nullary-variant pattern or a binding. A `Name(subpatterns)` form is
/// unambiguously a [`Pattern::Variant`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// `_` — matches anything, binds nothing.
    Wildcard { span: Span },
    /// A bare identifier: a binding, or a nullary variant (resolved later).
    Ident { name: String, span: Span },
    /// A variant pattern with sub-patterns, e.g. `Some(x)` or `Pair(a, b)`.
    Variant {
        name: String,
        args: Vec<Pattern>,
        span: Span,
    },
}

impl Pattern {
    /// The source span covering this pattern.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard { span }
            | Pattern::Ident { span, .. }
            | Pattern::Variant { span, .. } => *span,
        }
    }
}

/// A statement node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    /// A `let` binding. Immutable by default; `mutable` records an explicit
    /// opt-in to mutability.
    Let {
        name: String,
        mutable: bool,
        ty: Option<TypeExpr>,
        value: Expr,
        span: Span,
    },
    /// An expression evaluated for its effect or value.
    Expr(Expr),
    /// A `return`, optionally carrying a value.
    Return { value: Option<Expr>, span: Span },
    /// A conditional. The `else` branch is optional and, when written as
    /// `else if`, nests another `If` statement inside its block.
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },
}

/// A brace-delimited sequence of statements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

/// A type as written in source, e.g. `Int`, `Cap<Write>`, or `Tainted<Text>`.
///
/// This is the *syntactic* type; resolution and checking happen later in
/// `writ-check`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeExpr {
    /// The head name, e.g. `Cap` in `Cap<Write>`.
    pub name: String,
    /// Type arguments, e.g. `Write` in `Cap<Write>`.
    pub args: Vec<TypeExpr>,
    pub span: Span,
}

/// A single function parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}

/// One declared effect inside a `uses {...}` set, e.g. `Write` or `Net`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    pub name: String,
    pub span: Span,
}

/// The `uses {...}` effect set declared by a signature.
///
/// A signature with an empty effect set promises the function performs no
/// effects — the honesty check later verifies that promise against the body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EffectSet {
    pub effects: Vec<Effect>,
    /// Span of the whole `uses {...}` clause, if one was written.
    pub span: Option<Span>,
}

/// A single contract clause — the predicate of a `requires` or `ensures`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    /// The boolean predicate.
    pub predicate: Expr,
    /// Span of the whole clause, including the `requires` / `ensures` keyword.
    pub span: Span,
}

/// A function signature: the load-bearing surface that declares authority
/// (`uses`) and correctness (`requires` / `ensures`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub params: Vec<Param>,
    /// The declared return type, if written.
    pub return_type: Option<TypeExpr>,
    /// The declared effect set (`uses {...}`).
    pub effects: EffectSet,
    /// Preconditions — a failed one blames the caller.
    pub requires: Vec<Contract>,
    /// Postconditions — a failed one blames the implementation.
    pub ensures: Vec<Contract>,
    pub span: Span,
}

/// A function declaration: a signature paired with a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub signature: Signature,
    pub body: Block,
    pub span: Span,
}

/// One variant of a sum type, e.g. `Some(T)` or `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    /// Positional payload types, e.g. `[T]` for `Some(T)`; empty for a nullary
    /// variant like `None`.
    pub fields: Vec<TypeExpr>,
    pub span: Span,
}

/// A sum-type declaration, e.g. `type Option<T> = Some(T) | None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDecl {
    pub name: String,
    /// Generic type parameters, e.g. `["T"]` for `Option<T>`.
    pub generics: Vec<String>,
    pub variants: Vec<Variant>,
    pub span: Span,
}

/// A top-level item in a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A function declaration.
    Function(Function),
    /// A sum-type declaration.
    Type(TypeDecl),
}

/// A whole parsed source file: an ordered list of top-level items.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Module {
    pub items: Vec<Item>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp() -> Span {
        Span::new(0, 1)
    }

    #[test]
    fn expr_span_reports_own_span() {
        let e = Expr::Identifier {
            name: "x".into(),
            span: Span::new(4, 5),
        };
        assert_eq!(e.span(), Span::new(4, 5));

        let lit = Expr::Literal(Literal {
            kind: LiteralKind::Int(1),
            span: Span::new(2, 3),
        });
        assert_eq!(lit.span(), Span::new(2, 3));
    }

    #[test]
    fn binary_expression_nests() {
        // 1 + 2 * 3  parsed as  1 + (2 * 3)
        let mul = Expr::Binary {
            op: BinaryOp::Mul,
            left: Box::new(Expr::Literal(Literal {
                kind: LiteralKind::Int(2),
                span: sp(),
            })),
            right: Box::new(Expr::Literal(Literal {
                kind: LiteralKind::Int(3),
                span: sp(),
            })),
            span: sp(),
        };
        let add = Expr::Binary {
            op: BinaryOp::Add,
            left: Box::new(Expr::Literal(Literal {
                kind: LiteralKind::Int(1),
                span: sp(),
            })),
            right: Box::new(mul),
            span: sp(),
        };
        match add {
            Expr::Binary {
                op: BinaryOp::Add,
                right,
                ..
            } => {
                assert!(matches!(
                    *right,
                    Expr::Binary {
                        op: BinaryOp::Mul,
                        ..
                    }
                ));
            }
            _ => panic!("expected a top-level Add"),
        }
    }

    #[test]
    fn signature_has_room_for_effects_and_contracts() {
        // fn write_line(out: Cap<Write>, msg: Text) uses { Write }
        //   requires len(msg) > 0
        //   ensures true
        let sig = Signature {
            name: "write_line".into(),
            params: vec![
                Param {
                    name: "out".into(),
                    ty: TypeExpr {
                        name: "Cap".into(),
                        args: vec![TypeExpr {
                            name: "Write".into(),
                            args: vec![],
                            span: sp(),
                        }],
                        span: sp(),
                    },
                    span: sp(),
                },
                Param {
                    name: "msg".into(),
                    ty: TypeExpr {
                        name: "Text".into(),
                        args: vec![],
                        span: sp(),
                    },
                    span: sp(),
                },
            ],
            return_type: None,
            effects: EffectSet {
                effects: vec![Effect {
                    name: "Write".into(),
                    span: sp(),
                }],
                span: Some(sp()),
            },
            requires: vec![Contract {
                predicate: Expr::Literal(Literal {
                    kind: LiteralKind::Bool(true),
                    span: sp(),
                }),
                span: sp(),
            }],
            ensures: vec![Contract {
                predicate: Expr::Literal(Literal {
                    kind: LiteralKind::Bool(true),
                    span: sp(),
                }),
                span: sp(),
            }],
            span: sp(),
        };

        assert_eq!(sig.params.len(), 2);
        assert_eq!(sig.effects.effects.len(), 1);
        assert_eq!(sig.effects.effects[0].name, "Write");
        assert_eq!(sig.requires.len(), 1);
        assert_eq!(sig.ensures.len(), 1);
    }

    #[test]
    fn empty_effect_set_is_default() {
        let e = EffectSet::default();
        assert!(e.effects.is_empty());
        assert!(e.span.is_none());
    }

    #[test]
    fn module_collects_items() {
        let m = Module {
            items: vec![Item::Function(Function {
                signature: Signature {
                    name: "main".into(),
                    params: vec![],
                    return_type: None,
                    effects: EffectSet::default(),
                    requires: vec![],
                    ensures: vec![],
                    span: sp(),
                },
                body: Block {
                    stmts: vec![Stmt::Return {
                        value: None,
                        span: sp(),
                    }],
                    span: sp(),
                },
                span: sp(),
            })],
        };
        assert_eq!(m.items.len(), 1);
    }
}

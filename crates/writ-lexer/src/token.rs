//! Token types produced by the lexer.

use writ_ast::Span;

/// A reserved word. Kept distinct from identifiers so the parser never has to
/// re-classify by string comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Keyword {
    Fn,
    Let,
    Mut,
    Return,
    Uses,
    Requires,
    Ensures,
    If,
    Else,
    Match,
    True,
    False,
}

impl Keyword {
    /// Map an identifier string to a keyword, if it is one.
    #[must_use]
    pub fn lookup(s: &str) -> Option<Keyword> {
        Some(match s {
            "fn" => Keyword::Fn,
            "let" => Keyword::Let,
            "mut" => Keyword::Mut,
            "return" => Keyword::Return,
            "uses" => Keyword::Uses,
            "requires" => Keyword::Requires,
            "ensures" => Keyword::Ensures,
            "if" => Keyword::If,
            "else" => Keyword::Else,
            "match" => Keyword::Match,
            "true" => Keyword::True,
            "false" => Keyword::False,
            _ => return None,
        })
    }
}

/// The lexical category of a token, carrying any decoded payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    /// An integer literal, already parsed into its value.
    Int(i64),
    /// A text literal, with escape sequences already decoded.
    Text(String),
    /// An identifier that is not a keyword.
    Ident(String),
    /// A reserved word.
    Keyword(Keyword),

    // Operators.
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Eq,
    Bang,
    AmpAmp,
    PipePipe,
    /// `->`, the return-type arrow.
    Arrow,

    // Punctuation.
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semicolon,

    /// End of input. Carries a zero-width span at the end of the source.
    Eof,
}

/// A token: a lexical category paired with the exact byte span it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    #[must_use]
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

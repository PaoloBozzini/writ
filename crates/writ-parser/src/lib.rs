//! `writ-parser` — tokens → AST, via recursive descent.
//!
//! Expression parsing uses Pratt-style binding powers so operator precedence
//! and left-associativity are unambiguous: `1 + 2 * 3` parses as `1 + (2 * 3)`,
//! and grouping with parentheses overrides that shape. The parser also handles
//! `let` / `return` / expression statements and `fn` declarations.
//!
//! The `uses {...}` / `requires` / `ensures` signature clauses are added in
//! later work, as is error recovery past the first error; here a parse error
//! produces one structured [`Diagnostic`] and stops.

use writ_ast::{
    BinaryOp, Block, Expr, Function, Item, Literal, LiteralKind, Module, Param, Signature, Span,
    Stmt, TypeExpr, UnaryOp,
};
use writ_ast::{Diagnostic, EffectSet};
use writ_lexer::{Keyword, Token, TokenKind};

/// The result of parsing a source string: the (possibly partial) module and any
/// diagnostics from lexing and parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseResult {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse a whole source file into a [`Module`].
#[must_use]
pub fn parse(source: &str) -> ParseResult {
    let lexed = writ_lexer::lex(source);
    let mut diagnostics = lexed.diagnostics;
    let mut parser = Parser::new(&lexed.tokens);
    let module = parser.parse_module();
    diagnostics.extend(parser.diagnostics);
    ParseResult {
        module,
        diagnostics,
    }
}

/// Parse a single expression. Convenience for testing precedence in isolation.
///
/// # Errors
/// Returns the diagnostics produced if the source does not lex and parse into
/// exactly one expression followed by end of input.
pub fn parse_expr(source: &str) -> Result<Expr, Vec<Diagnostic>> {
    let lexed = writ_lexer::lex(source);
    if !lexed.diagnostics.is_empty() {
        return Err(lexed.diagnostics);
    }
    let mut parser = Parser::new(&lexed.tokens);
    match parser.expression() {
        Ok(expr) if parser.at_end() && parser.diagnostics.is_empty() => Ok(expr),
        Ok(_) => {
            let trailing = parser.error_here("P0003", "unexpected trailing input after expression");
            let mut diags = parser.diagnostics;
            diags.push(trailing);
            Err(diags)
        }
        Err(d) => {
            let mut diags = parser.diagnostics;
            diags.push(d);
            Err(diags)
        }
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    diagnostics: Vec<Diagnostic>,
}

/// Left binding power for each binary operator. Higher binds tighter. Absent
/// tokens are not binary operators.
fn binding_power(kind: &TokenKind) -> Option<(BinaryOp, u8)> {
    Some(match kind {
        TokenKind::PipePipe => (BinaryOp::Or, 1),
        TokenKind::AmpAmp => (BinaryOp::And, 2),
        TokenKind::EqEq => (BinaryOp::Eq, 3),
        TokenKind::NotEq => (BinaryOp::Ne, 3),
        TokenKind::Lt => (BinaryOp::Lt, 4),
        TokenKind::LtEq => (BinaryOp::Le, 4),
        TokenKind::Gt => (BinaryOp::Gt, 4),
        TokenKind::GtEq => (BinaryOp::Ge, 4),
        TokenKind::Plus => (BinaryOp::Add, 5),
        TokenKind::Minus => (BinaryOp::Sub, 5),
        TokenKind::Star => (BinaryOp::Mul, 6),
        TokenKind::Slash => (BinaryOp::Div, 6),
        TokenKind::Percent => (BinaryOp::Rem, 6),
        _ => return None,
    })
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            pos: 0,
            diagnostics: Vec::new(),
        }
    }

    fn peek(&self) -> &TokenKind {
        self.tokens
            .get(self.pos)
            .map_or(&TokenKind::Eof, |t| &t.kind)
    }

    fn peek_span(&self) -> Span {
        match self.tokens.get(self.pos) {
            Some(t) => t.span,
            None => self.tokens.last().map_or(Span::new(0, 0), |t| t.span),
        }
    }

    fn at_end(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    fn advance(&mut self) -> &'a Token {
        let tok = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn error_here(&self, code: &str, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(code, self.peek_span(), message)
    }

    /// Consume a token of the expected kind or produce a diagnostic.
    fn expect(&mut self, want: &TokenKind, what: &str) -> Result<Span, Diagnostic> {
        if self.peek() == want {
            Ok(self.advance().span)
        } else {
            Err(self.error_here(
                "P0001",
                format!("expected {what}, found {}", describe(self.peek())),
            ))
        }
    }

    // --- Items -------------------------------------------------------------

    fn parse_module(&mut self) -> Module {
        let mut items = Vec::new();
        while !self.at_end() {
            match self.parse_item() {
                Ok(item) => items.push(item),
                Err(d) => {
                    self.diagnostics.push(d);
                    break; // No recovery yet; that is a later milestone.
                }
            }
        }
        Module { items }
    }

    fn parse_item(&mut self) -> Result<Item, Diagnostic> {
        match self.peek() {
            TokenKind::Keyword(Keyword::Fn) => self.parse_function().map(Item::Function),
            _ => Err(self.error_here("P0002", "expected an item (a `fn` declaration)")),
        }
    }

    fn parse_function(&mut self) -> Result<Function, Diagnostic> {
        let start = self.expect(&TokenKind::Keyword(Keyword::Fn), "`fn`")?;
        let name = self.expect_ident("a function name")?;
        self.expect(&TokenKind::LParen, "`(`")?;
        let params = self.parse_params()?;
        self.expect(&TokenKind::RParen, "`)`")?;

        let return_type = if self.peek() == &TokenKind::Arrow {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        // The signature spans `fn` through the last token before the body.
        let sig_end = self.tokens[self.pos.saturating_sub(1)].span;
        let body = self.parse_block()?;
        let span = start.merge(&body.span);
        let signature = Signature {
            name,
            params,
            return_type,
            effects: EffectSet::default(),
            requires: Vec::new(),
            ensures: Vec::new(),
            span: start.merge(&sig_end),
        };
        Ok(Function {
            signature,
            body,
            span,
        })
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, Diagnostic> {
        let mut params = Vec::new();
        while self.peek() != &TokenKind::RParen {
            let (name, name_span) = self.expect_ident_spanned("a parameter name")?;
            self.expect(&TokenKind::Colon, "`:`")?;
            let ty = self.parse_type()?;
            let span = name_span.merge(&ty.span);
            params.push(Param { name, ty, span });
            if self.peek() == &TokenKind::Comma {
                self.advance();
            } else {
                break;
            }
        }
        Ok(params)
    }

    fn parse_type(&mut self) -> Result<TypeExpr, Diagnostic> {
        let (name, span) = self.expect_ident_spanned("a type name")?;
        let mut end = span;
        let mut args = Vec::new();
        if self.peek() == &TokenKind::Lt {
            self.advance();
            loop {
                args.push(self.parse_type()?);
                match self.peek() {
                    TokenKind::Comma => {
                        self.advance();
                    }
                    _ => break,
                }
            }
            end = self.expect(&TokenKind::Gt, "`>` to close the type arguments")?;
        }
        Ok(TypeExpr {
            name,
            args,
            span: span.merge(&end),
        })
    }

    fn parse_block(&mut self) -> Result<Block, Diagnostic> {
        let start = self.expect(&TokenKind::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        while self.peek() != &TokenKind::RBrace && !self.at_end() {
            stmts.push(self.parse_stmt()?);
        }
        let end = self.expect(&TokenKind::RBrace, "`}`")?;
        Ok(Block {
            stmts,
            span: start.merge(&end),
        })
    }

    // --- Statements --------------------------------------------------------

    fn parse_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        match self.peek() {
            TokenKind::Keyword(Keyword::Let) => self.parse_let(),
            TokenKind::Keyword(Keyword::Return) => self.parse_return(),
            _ => {
                let expr = self.expression()?;
                self.expect(&TokenKind::Semicolon, "`;` after the expression")?;
                Ok(Stmt::Expr(expr))
            }
        }
    }

    fn parse_let(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.advance().span; // `let`
        let mutable = if self.peek() == &TokenKind::Keyword(Keyword::Mut) {
            self.advance();
            true
        } else {
            false
        };
        let name = self.expect_ident("a binding name")?;
        let ty = if self.peek() == &TokenKind::Colon {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&TokenKind::Eq, "`=` in the `let` binding")?;
        let value = self.expression()?;
        let end = self.expect(&TokenKind::Semicolon, "`;` after the `let` binding")?;
        Ok(Stmt::Let {
            name,
            mutable,
            ty,
            value,
            span: start.merge(&end),
        })
    }

    fn parse_return(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.advance().span; // `return`
        if self.peek() == &TokenKind::Semicolon {
            let end = self.advance().span;
            return Ok(Stmt::Return {
                value: None,
                span: start.merge(&end),
            });
        }
        let value = self.expression()?;
        let end = self.expect(&TokenKind::Semicolon, "`;` after the `return` value")?;
        Ok(Stmt::Return {
            value: Some(value),
            span: start.merge(&end),
        })
    }

    // --- Expressions -------------------------------------------------------

    fn expression(&mut self) -> Result<Expr, Diagnostic> {
        self.expr_bp(0)
    }

    /// Pratt parser: parse operators whose binding power is at least `min_bp`.
    fn expr_bp(&mut self, min_bp: u8) -> Result<Expr, Diagnostic> {
        let mut lhs = self.unary()?;
        while let Some((op, bp)) = binding_power(self.peek()) {
            if bp < min_bp {
                break;
            }
            self.advance(); // operator
            let rhs = self.expr_bp(bp + 1)?; // left-associative
            let span = lhs.span().merge(&rhs.span());
            lhs = Expr::Binary {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, Diagnostic> {
        let op = match self.peek() {
            TokenKind::Minus => Some(UnaryOp::Neg),
            TokenKind::Bang => Some(UnaryOp::Not),
            _ => None,
        };
        if let Some(op) = op {
            let start = self.advance().span;
            let operand = self.unary()?;
            let span = start.merge(&operand.span());
            Ok(Expr::Unary {
                op,
                operand: Box::new(operand),
                span,
            })
        } else {
            self.postfix()
        }
    }

    /// Primary expressions followed by any number of call suffixes.
    fn postfix(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.primary()?;
        while self.peek() == &TokenKind::LParen {
            self.advance(); // `(`
            let mut args = Vec::new();
            while self.peek() != &TokenKind::RParen {
                args.push(self.expression()?);
                if self.peek() == &TokenKind::Comma {
                    self.advance();
                } else {
                    break;
                }
            }
            let end = self.expect(&TokenKind::RParen, "`)` to close the call arguments")?;
            let span = expr.span().merge(&end);
            expr = Expr::Call {
                callee: Box::new(expr),
                args,
                span,
            };
        }
        Ok(expr)
    }

    fn primary(&mut self) -> Result<Expr, Diagnostic> {
        let span = self.peek_span();
        match self.peek().clone() {
            TokenKind::Int(v) => {
                self.advance();
                Ok(Expr::Literal(Literal {
                    kind: LiteralKind::Int(v),
                    span,
                }))
            }
            TokenKind::Text(s) => {
                self.advance();
                Ok(Expr::Literal(Literal {
                    kind: LiteralKind::Text(s),
                    span,
                }))
            }
            TokenKind::Keyword(Keyword::True) => {
                self.advance();
                Ok(Expr::Literal(Literal {
                    kind: LiteralKind::Bool(true),
                    span,
                }))
            }
            TokenKind::Keyword(Keyword::False) => {
                self.advance();
                Ok(Expr::Literal(Literal {
                    kind: LiteralKind::Bool(false),
                    span,
                }))
            }
            TokenKind::Ident(name) => {
                self.advance();
                Ok(Expr::Identifier { name, span })
            }
            TokenKind::LParen => {
                self.advance();
                let inner = self.expr_bp(0)?;
                self.expect(&TokenKind::RParen, "`)` to close the group")?;
                Ok(inner)
            }
            other => Err(self.error_here(
                "P0002",
                format!("expected an expression, found {}", describe(&other)),
            )),
        }
    }

    // --- Small helpers -----------------------------------------------------

    fn expect_ident(&mut self, what: &str) -> Result<String, Diagnostic> {
        self.expect_ident_spanned(what).map(|(name, _)| name)
    }

    fn expect_ident_spanned(&mut self, what: &str) -> Result<(String, Span), Diagnostic> {
        if let TokenKind::Ident(name) = self.peek().clone() {
            let span = self.advance().span;
            Ok((name, span))
        } else {
            Err(self.error_here(
                "P0001",
                format!("expected {what}, found {}", describe(self.peek())),
            ))
        }
    }
}

/// A short, stable description of a token kind for diagnostic messages.
fn describe(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Int(_) => "an integer literal".into(),
        TokenKind::Text(_) => "a text literal".into(),
        TokenKind::Ident(_) => "an identifier".into(),
        TokenKind::Keyword(_) => "a keyword".into(),
        TokenKind::Eof => "end of input".into(),
        other => format!("`{}`", symbol(other)),
    }
}

fn symbol(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::Plus => "+",
        TokenKind::Minus => "-",
        TokenKind::Star => "*",
        TokenKind::Slash => "/",
        TokenKind::Percent => "%",
        TokenKind::EqEq => "==",
        TokenKind::NotEq => "!=",
        TokenKind::Lt => "<",
        TokenKind::LtEq => "<=",
        TokenKind::Gt => ">",
        TokenKind::GtEq => ">=",
        TokenKind::Eq => "=",
        TokenKind::Bang => "!",
        TokenKind::AmpAmp => "&&",
        TokenKind::PipePipe => "||",
        TokenKind::Arrow => "->",
        TokenKind::LParen => "(",
        TokenKind::RParen => ")",
        TokenKind::LBrace => "{",
        TokenKind::RBrace => "}",
        TokenKind::Comma => ",",
        TokenKind::Colon => ":",
        TokenKind::Semicolon => ";",
        _ => "token",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op_of(e: &Expr) -> Option<BinaryOp> {
        match e {
            Expr::Binary { op, .. } => Some(*op),
            _ => None,
        }
    }

    #[test]
    fn precedence_multiplication_binds_tighter_than_addition() {
        // 1 + 2 * 3  ==  1 + (2 * 3)
        let e = parse_expr("1 + 2 * 3").unwrap();
        assert_eq!(op_of(&e), Some(BinaryOp::Add));
        if let Expr::Binary { right, .. } = e {
            assert_eq!(op_of(&right), Some(BinaryOp::Mul));
        } else {
            panic!("expected Add at the root");
        }
    }

    #[test]
    fn parentheses_override_precedence() {
        // (1 + 2) * 3  ==  (1 + 2) * 3, Mul at root
        let e = parse_expr("(1 + 2) * 3").unwrap();
        assert_eq!(op_of(&e), Some(BinaryOp::Mul));
        if let Expr::Binary { left, .. } = e {
            assert_eq!(op_of(&left), Some(BinaryOp::Add));
        } else {
            panic!("expected Mul at the root");
        }
    }

    #[test]
    fn subtraction_is_left_associative() {
        // 10 - 3 - 2  ==  (10 - 3) - 2
        let e = parse_expr("10 - 3 - 2").unwrap();
        assert_eq!(op_of(&e), Some(BinaryOp::Sub));
        if let Expr::Binary { left, right, .. } = e {
            assert_eq!(op_of(&left), Some(BinaryOp::Sub));
            assert!(matches!(
                *right,
                Expr::Literal(Literal {
                    kind: LiteralKind::Int(2),
                    ..
                })
            ));
        } else {
            panic!("expected Sub at the root");
        }
    }

    #[test]
    fn comparison_binds_looser_than_arithmetic() {
        // 1 + 2 < 3 * 4  ==  (1 + 2) < (3 * 4)
        let e = parse_expr("1 + 2 < 3 * 4").unwrap();
        assert_eq!(op_of(&e), Some(BinaryOp::Lt));
    }

    #[test]
    fn unary_and_call_parse() {
        let e = parse_expr("-f(1, 2)").unwrap();
        assert!(matches!(
            e,
            Expr::Unary {
                op: UnaryOp::Neg,
                ..
            }
        ));
    }

    #[test]
    fn round_trips_a_sample_program() {
        let source = "\
fn add(a: Int, b: Int) -> Int {
    let sum = a + b;
    return sum;
}
";
        let result = parse(source);
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        assert_eq!(result.module.items.len(), 1);
        let Item::Function(f) = &result.module.items[0];
        assert_eq!(f.signature.name, "add");
        assert_eq!(f.signature.params.len(), 2);
        assert_eq!(f.signature.params[0].ty.name, "Int");
        assert_eq!(f.signature.return_type.as_ref().unwrap().name, "Int");
        assert_eq!(f.body.stmts.len(), 2);
        assert!(matches!(f.body.stmts[0], Stmt::Let { .. }));
        assert!(matches!(
            f.body.stmts[1],
            Stmt::Return { value: Some(_), .. }
        ));
    }

    #[test]
    fn parses_generic_type_argument() {
        let source = "fn write(out: Cap<Write>) { return; }";
        let result = parse(source);
        assert!(
            result.diagnostics.is_empty(),
            "diagnostics: {:?}",
            result.diagnostics
        );
        let Item::Function(f) = &result.module.items[0];
        let ty = &f.signature.params[0].ty;
        assert_eq!(ty.name, "Cap");
        assert_eq!(ty.args.len(), 1);
        assert_eq!(ty.args[0].name, "Write");
    }

    // --- Negative tests: malformed input is refused with a diagnostic.

    #[test]
    fn missing_operand_is_refused() {
        let err = parse_expr("1 +").unwrap_err();
        assert_eq!(err[0].code, "P0002");
    }

    #[test]
    fn unclosed_paren_is_refused() {
        let err = parse_expr("(1 + 2").unwrap_err();
        assert_eq!(err[0].code, "P0001");
    }

    #[test]
    fn non_item_at_top_level_is_refused() {
        let result = parse("let x = 1;");
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].code, "P0002");
    }
}

//! `writ-lexer` — text → tokens, the first stage of the pipeline.
//!
//! Every token carries a byte [`Span`] so later stages can anchor deterministic,
//! machine-readable diagnostics to exact source locations. The lexer never
//! panics on malformed input: unknown characters and malformed literals produce
//! a structured [`Diagnostic`] and the scan recovers, so one bad byte can't hide
//! the rest of the errors in a file.

mod token;

use writ_ast::{Diagnostic, Span};

pub use token::{Keyword, Token, TokenKind};

/// The result of lexing a source string: the tokens, and any diagnostics
/// produced along the way. A non-empty `diagnostics` means the source was
/// malformed, but `tokens` still holds everything that lexed cleanly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexResult {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Tokenize `source` into a stream of spanned tokens, terminated by an
/// [`TokenKind::Eof`] token with a zero-width span at the end of input.
#[must_use]
pub fn lex(source: &str) -> LexResult {
    Lexer::new(source).run()
}

struct Lexer<'a> {
    src: &'a str,
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            chars: src.char_indices().peekable(),
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self) -> LexResult {
        while let Some((start, ch)) = self.chars.next() {
            self.scan(start, ch);
        }
        let end = self.src.len();
        self.tokens
            .push(Token::new(TokenKind::Eof, Span::new(end, end)));
        LexResult {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        }
    }

    /// Peek the next char and its byte offset without consuming it.
    fn peek(&mut self) -> Option<(usize, char)> {
        self.chars.peek().copied()
    }

    /// Consume the next char if it equals `want`; report success.
    fn eat(&mut self, want: char) -> bool {
        match self.peek() {
            Some((_, c)) if c == want => {
                self.chars.next();
                true
            }
            _ => false,
        }
    }

    /// Consume chars while `pred` holds, starting from `first_end` (the byte
    /// offset just past the already-consumed first char). Returns the byte
    /// offset one past the last consumed char.
    fn consume_while(&mut self, first_end: usize, pred: impl Fn(char) -> bool) -> usize {
        let mut end = first_end;
        while let Some((off, c)) = self.peek() {
            if pred(c) {
                self.chars.next();
                end = off + c.len_utf8();
            } else {
                break;
            }
        }
        end
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        self.tokens.push(Token::new(kind, Span::new(start, end)));
    }

    fn scan(&mut self, start: usize, ch: char) {
        let one = start + ch.len_utf8();
        match ch {
            c if c.is_whitespace() => {}
            '/' if matches!(self.peek(), Some((_, '/'))) => {
                // Line comment: skip to end of line.
                self.consume_while(one, |c| c != '\n');
            }
            '+' => self.push(TokenKind::Plus, start, one),
            '-' => {
                if self.eat('>') {
                    self.push(TokenKind::Arrow, start, one + 1);
                } else {
                    self.push(TokenKind::Minus, start, one);
                }
            }
            '*' => self.push(TokenKind::Star, start, one),
            '/' => self.push(TokenKind::Slash, start, one),
            '%' => self.push(TokenKind::Percent, start, one),
            '(' => self.push(TokenKind::LParen, start, one),
            ')' => self.push(TokenKind::RParen, start, one),
            '{' => self.push(TokenKind::LBrace, start, one),
            '}' => self.push(TokenKind::RBrace, start, one),
            ',' => self.push(TokenKind::Comma, start, one),
            ':' => self.push(TokenKind::Colon, start, one),
            ';' => self.push(TokenKind::Semicolon, start, one),
            '=' => {
                if self.eat('=') {
                    self.push(TokenKind::EqEq, start, one + 1);
                } else {
                    self.push(TokenKind::Eq, start, one);
                }
            }
            '!' => {
                if self.eat('=') {
                    self.push(TokenKind::NotEq, start, one + 1);
                } else {
                    self.push(TokenKind::Bang, start, one);
                }
            }
            '<' => {
                if self.eat('=') {
                    self.push(TokenKind::LtEq, start, one + 1);
                } else {
                    self.push(TokenKind::Lt, start, one);
                }
            }
            '>' => {
                if self.eat('=') {
                    self.push(TokenKind::GtEq, start, one + 1);
                } else {
                    self.push(TokenKind::Gt, start, one);
                }
            }
            '&' if self.eat('&') => self.push(TokenKind::AmpAmp, start, one + 1),
            '|' if self.eat('|') => self.push(TokenKind::PipePipe, start, one + 1),
            '"' => self.scan_text(start),
            c if c.is_ascii_digit() => self.scan_number(start, one),
            c if c.is_ascii_alphabetic() || c == '_' => self.scan_ident(start, one),
            c => {
                let end = start + c.len_utf8();
                self.diagnostics.push(Diagnostic::error(
                    "L0001",
                    Span::new(start, end),
                    format!("unknown character `{c}`"),
                ));
            }
        }
    }

    fn scan_number(&mut self, start: usize, first_end: usize) {
        let end = self.consume_while(first_end, |c| c.is_ascii_digit());
        let text = &self.src[start..end];
        match text.parse::<i64>() {
            Ok(value) => self.push(TokenKind::Int(value), start, end),
            Err(_) => self.diagnostics.push(Diagnostic::error(
                "L0002",
                Span::new(start, end),
                format!("integer literal `{text}` does not fit in a 64-bit integer"),
            )),
        }
    }

    fn scan_ident(&mut self, start: usize, first_end: usize) {
        let end = self.consume_while(first_end, |c| c.is_ascii_alphanumeric() || c == '_');
        let text = &self.src[start..end];
        let kind = match Keyword::lookup(text) {
            Some(kw) => TokenKind::Keyword(kw),
            None => TokenKind::Ident(text.to_string()),
        };
        self.push(kind, start, end);
    }

    fn scan_text(&mut self, start: usize) {
        let mut value = String::new();
        loop {
            match self.chars.next() {
                None => {
                    self.diagnostics.push(Diagnostic::error(
                        "L0003",
                        Span::new(start, self.src.len()),
                        "unterminated text literal",
                    ));
                    return;
                }
                Some((off, '"')) => {
                    self.push(TokenKind::Text(value), start, off + 1);
                    return;
                }
                Some((off, '\\')) => match self.chars.next() {
                    Some((_, 'n')) => value.push('\n'),
                    Some((_, 't')) => value.push('\t'),
                    Some((_, '"')) => value.push('"'),
                    Some((_, '\\')) => value.push('\\'),
                    Some((esc_off, c)) => {
                        self.diagnostics.push(Diagnostic::error(
                            "L0004",
                            Span::new(off, esc_off + c.len_utf8()),
                            format!("invalid escape sequence `\\{c}`"),
                        ));
                        value.push(c);
                    }
                    None => {
                        self.diagnostics.push(Diagnostic::error(
                            "L0003",
                            Span::new(start, self.src.len()),
                            "unterminated text literal",
                        ));
                        return;
                    }
                },
                Some((_, c)) => value.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        lex(source).tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn tokenizes_each_category() {
        let result = lex("let x = 42 + foo");
        let kinds: Vec<_> = result.tokens.iter().map(|t| &t.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &TokenKind::Keyword(Keyword::Let),
                &TokenKind::Ident("x".into()),
                &TokenKind::Eq,
                &TokenKind::Int(42),
                &TokenKind::Plus,
                &TokenKind::Ident("foo".into()),
                &TokenKind::Eof,
            ]
        );
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn keywords_are_not_identifiers() {
        assert_eq!(
            kinds("fn requires ensures uses"),
            vec![
                TokenKind::Keyword(Keyword::Fn),
                TokenKind::Keyword(Keyword::Requires),
                TokenKind::Keyword(Keyword::Ensures),
                TokenKind::Keyword(Keyword::Uses),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn multi_char_operators() {
        assert_eq!(
            kinds("== != <= >= -> && ||"),
            vec![
                TokenKind::EqEq,
                TokenKind::NotEq,
                TokenKind::LtEq,
                TokenKind::GtEq,
                TokenKind::Arrow,
                TokenKind::AmpAmp,
                TokenKind::PipePipe,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn every_token_carries_a_byte_span() {
        // 0123456
        // foo bar
        let toks = lex("foo bar").tokens;
        assert_eq!(toks[0].span, Span::new(0, 3));
        assert_eq!(toks[1].span, Span::new(4, 7));
        // Eof is a zero-width span at end of input.
        assert_eq!(toks[2].span, Span::new(7, 7));
    }

    #[test]
    fn text_literal_decodes_escapes() {
        let toks = lex(r#""a\n\"b""#).tokens;
        assert_eq!(toks[0].kind, TokenKind::Text("a\n\"b".to_string()));
    }

    #[test]
    fn line_comments_are_skipped() {
        assert_eq!(
            kinds("1 // ignored\n2"),
            vec![TokenKind::Int(1), TokenKind::Int(2), TokenKind::Eof]
        );
    }

    // --- Negative tests: malformed input is refused with a diagnostic, never a panic.

    #[test]
    fn unknown_character_is_a_structured_error_not_a_panic() {
        let result = lex("a # b");
        assert_eq!(result.diagnostics.len(), 1);
        let d = &result.diagnostics[0];
        assert_eq!(d.code, "L0001");
        assert_eq!(d.span, Span::new(2, 3));
        assert!(d.is_error());
        // The scan recovered: tokens on both sides of the bad char are present.
        assert_eq!(
            result.tokens.iter().map(|t| &t.kind).collect::<Vec<_>>(),
            vec![
                &TokenKind::Ident("a".into()),
                &TokenKind::Ident("b".into()),
                &TokenKind::Eof
            ]
        );
    }

    #[test]
    fn integer_overflow_is_refused() {
        let result = lex("99999999999999999999999999");
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].code, "L0002");
    }

    #[test]
    fn unterminated_text_literal_is_refused() {
        let result = lex("\"no close");
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].code, "L0003");
    }

    #[test]
    fn unicode_unknown_char_does_not_panic_on_byte_boundary() {
        // A multi-byte char must produce a span on a char boundary, not panic.
        let result = lex("é");
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].code, "L0001");
        assert_eq!(result.diagnostics[0].span, Span::new(0, 2));
    }
}

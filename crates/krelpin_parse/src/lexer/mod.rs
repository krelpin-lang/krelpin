mod literal;

#[cfg(test)]
mod tests;

use krelpin_ast::token::{Token, TokenKind};
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_lexer::{self as raw, Cursor};
use krelpin_span::{BytePos, SourceFile, Span, Symbol};

pub fn lex(file: &SourceFile, dcx: &DiagCtxt) -> Vec<Token> {
    lex_str(&file.src, file.start_pos, dcx)
}

pub fn lex_str(src: &str, start_pos: BytePos, dcx: &DiagCtxt) -> Vec<Token> {
    let mut reader = StringReader::new(src, start_pos, dcx);
    let mut tokens: Vec<Token> = Vec::new();

    loop {
        let (token, preceded_by_whitespace) = reader.next_token();

        let glued = tokens
            .last()
            .filter(|_| !preceded_by_whitespace)
            .and_then(|last| last.glue(&token));
        if let Some(glued) = glued {
            tokens.pop();
            tokens.push(glued);
            continue;
        }

        let is_eof = token.kind == TokenKind::Eof;
        tokens.push(token);
        if is_eof {
            return tokens;
        }
    }
}

struct StringReader<'a> {
    src: &'a str,
    start_pos: BytePos,
    pos: BytePos,
    cursor: Cursor<'a>,
    dcx: &'a DiagCtxt,
}

impl<'a> StringReader<'a> {
    fn new(src: &'a str, start_pos: BytePos, dcx: &'a DiagCtxt) -> StringReader<'a> {
        StringReader {
            src,
            start_pos,
            pos: start_pos,
            cursor: Cursor::new(src),
            dcx,
        }
    }

    fn next_token(&mut self) -> (Token, bool) {
        let mut preceded_by_whitespace = false;
        loop {
            let token = self.cursor.advance_token();
            let start = self.pos;
            self.pos = self.pos + token.len;

            match self.cook(start, token.kind) {
                Some(kind) => {
                    let span = Span::new(start, self.pos);
                    return (Token::new(kind, span), preceded_by_whitespace);
                }
                None => preceded_by_whitespace = true,
            }
        }
    }

    fn cook(&self, start: BytePos, kind: raw::TokenKind) -> Option<TokenKind> {
        let kind = match kind {
            raw::TokenKind::Whitespace | raw::TokenKind::LineComment => return None,
            raw::TokenKind::BlockComment { doc, terminated } => {
                return self.cook_block_comment(start, doc, terminated);
            }
            raw::TokenKind::Unknown => {
                self.report_unknown_char(start);
                return None;
            }
            raw::TokenKind::Ident => TokenKind::Ident(Symbol::intern(self.str_from(start))),
            raw::TokenKind::Literal { kind, suffix_start } => {
                TokenKind::Literal(self.cook_literal(start, kind, suffix_start))
            }
            raw::TokenKind::Semi => TokenKind::Semi,
            raw::TokenKind::Comma => TokenKind::Comma,
            raw::TokenKind::Dot => TokenKind::Dot,
            raw::TokenKind::OpenParen => TokenKind::OpenParen,
            raw::TokenKind::CloseParen => TokenKind::CloseParen,
            raw::TokenKind::OpenBrace => TokenKind::OpenBrace,
            raw::TokenKind::CloseBrace => TokenKind::CloseBrace,
            raw::TokenKind::OpenBracket => TokenKind::OpenBracket,
            raw::TokenKind::CloseBracket => TokenKind::CloseBracket,
            raw::TokenKind::At => TokenKind::At,
            raw::TokenKind::Tilde => TokenKind::Tilde,
            raw::TokenKind::Question => TokenKind::Question,
            raw::TokenKind::Colon => TokenKind::Colon,
            raw::TokenKind::Eq => TokenKind::Eq,
            raw::TokenKind::Bang => TokenKind::Bang,
            raw::TokenKind::Lt => TokenKind::Lt,
            raw::TokenKind::Gt => TokenKind::Gt,
            raw::TokenKind::Minus => TokenKind::Minus,
            raw::TokenKind::And => TokenKind::And,
            raw::TokenKind::Or => TokenKind::Or,
            raw::TokenKind::Plus => TokenKind::Plus,
            raw::TokenKind::Star => TokenKind::Star,
            raw::TokenKind::Slash => TokenKind::Slash,
            raw::TokenKind::Caret => TokenKind::Caret,
            raw::TokenKind::Percent => TokenKind::Percent,
            raw::TokenKind::Eof => TokenKind::Eof,
        };
        Some(kind)
    }

    fn cook_block_comment(&self, start: BytePos, doc: bool, terminated: bool) -> Option<TokenKind> {
        if !terminated {
            self.dcx.emit(
                Diagnostic::error("unterminated block comment")
                    .with_span(Span::new(start, start + 2))
                    .with_label("comment starts here")
                    .with_help("close the comment with `*/`"),
            );
            return None;
        }

        if !doc {
            return None;
        }

        let text = self.str_from(start);
        Some(TokenKind::DocComment(Symbol::intern(
            &text[3..text.len() - 2],
        )))
    }

    fn report_unknown_char(&self, start: BytePos) {
        let c = self.str_from(start).chars().next().unwrap_or_default();
        self.dcx.emit(
            Diagnostic::error(format!("unknown start of token: {}", c.escape_debug()))
                .with_span(Span::new(start, self.pos)),
        );
    }

    fn str_from(&self, start: BytePos) -> &'a str {
        self.str_from_to(start, self.pos)
    }

    fn str_from_to(&self, lo: BytePos, hi: BytePos) -> &'a str {
        &self.src[(lo - self.start_pos) as usize..(hi - self.start_pos) as usize]
    }
}

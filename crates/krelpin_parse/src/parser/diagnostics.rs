use std::fmt;

use krelpin_ast::token::{LitKind, Token, TokenKind};
use krelpin_errors::Diagnostic;
use krelpin_span::Span;

use super::{ErrorReported, Expected, Parser};

impl fmt::Display for Expected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expected::Token(kind) => write!(f, "`{}`", kind.as_str().unwrap_or("?")),
            Expected::Keyword(keyword) => write!(f, "`{keyword}`"),
            Expected::Ident => f.write_str("identifier"),
            Expected::Type => f.write_str("type"),
            Expected::Expr => f.write_str("expression"),
            Expected::Item => f.write_str("type declaration"),
        }
    }
}

impl Parser<'_> {
    pub(crate) fn unexpected(&mut self) -> ErrorReported {
        let message = self.unexpected_message();
        self.error(self.token.span, message)
    }

    pub(crate) fn unexpected_message(&self) -> String {
        let found = describe(self.token);
        match expected_list(&self.expected) {
            Some(expected) => format!("expected {expected}, found {found}"),
            None => format!("unexpected {found}"),
        }
    }

    pub(crate) fn error(&self, span: Span, message: impl Into<String>) -> ErrorReported {
        self.emit(Diagnostic::error(message).with_span(span))
    }

    pub(crate) fn emit(&self, diagnostic: Diagnostic) -> ErrorReported {
        self.dcx.emit(diagnostic);
        ErrorReported
    }

    pub(crate) fn unclosed_delimiter(&self, open: Span) -> ErrorReported {
        self.emit(
            Diagnostic::error("this file contains an unclosed delimiter")
                .with_span(open)
                .with_label("unclosed delimiter"),
        )
    }

    pub(crate) fn recover_to_member_end(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.token.kind {
                TokenKind::Eof => return,
                TokenKind::Semi if depth == 0 => {
                    self.bump();
                    return;
                }
                TokenKind::CloseBrace if depth == 0 => return,
                TokenKind::CloseBrace if depth == 1 => {
                    self.bump();
                    return;
                }
                kind if is_open_delimiter(kind) => depth += 1,
                kind if is_close_delimiter(kind) => depth = depth.saturating_sub(1),
                _ => {}
            }
            self.bump();
        }
    }

    pub(crate) fn recover_to_next_item(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.token.kind {
                TokenKind::Eof => return,
                TokenKind::At if depth == 0 => return,
                TokenKind::Ident(symbol) if depth == 0 && starts_item(symbol) => return,
                kind if is_open_delimiter(kind) => depth += 1,
                kind if is_close_delimiter(kind) => depth = depth.saturating_sub(1),
                _ => {}
            }
            self.bump();
        }
    }
}

fn starts_item(symbol: krelpin_span::Symbol) -> bool {
    use krelpin_span::kw;
    matches!(
        symbol,
        kw::Class
            | kw::Record
            | kw::Struct
            | kw::Interface
            | kw::Enum
            | kw::Abstract
            | kw::Final
            | kw::Public
            | kw::Private
            | kw::Protected
    )
}

fn is_open_delimiter(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::OpenParen | TokenKind::OpenBrace | TokenKind::OpenBracket
    )
}

fn is_close_delimiter(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::CloseParen | TokenKind::CloseBrace | TokenKind::CloseBracket
    )
}

fn expected_list(expected: &[Expected]) -> Option<String> {
    let mut unique: Vec<String> = Vec::new();
    for item in expected {
        let text = item.to_string();
        if !unique.contains(&text) {
            unique.push(text);
        }
    }

    match unique.as_slice() {
        [] => None,
        [one] => Some(one.clone()),
        [first, second] => Some(format!("{first} or {second}")),
        [rest @ .., last] => Some(format!("one of {}, or {last}", rest.join(", "))),
    }
}

pub(crate) fn describe(token: Token) -> String {
    match token.kind {
        TokenKind::Ident(symbol) if symbol.is_keyword() => format!("keyword `{symbol}`"),
        TokenKind::Ident(symbol) => format!("`{symbol}`"),
        TokenKind::Literal(lit) => {
            let suffix = lit.suffix.map_or("", |suffix| suffix.as_str());
            match lit.kind {
                LitKind::Str => format!("`\"{}\"`", lit.symbol),
                LitKind::Char => format!("`'{}'`", lit.symbol),
                LitKind::TextBlock => "text block".to_owned(),
                LitKind::Integer | LitKind::Float | LitKind::Err => {
                    format!("`{}{suffix}`", lit.symbol)
                }
            }
        }
        TokenKind::DocComment(_) => "doc comment".to_owned(),
        TokenKind::Eof => "end of file".to_owned(),
        kind => format!("`{}`", kind.as_str().unwrap_or("?")),
    }
}

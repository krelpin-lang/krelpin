use krelpin_ast::ast::PrimitiveTy;
use krelpin_ast::token::TokenKind;
use krelpin_span::kw;

use super::Parser;

impl Parser<'_> {
    pub(crate) fn lambda_ahead(&self) -> bool {
        match self.token.kind {
            kind if is_plain_ident(kind) => self.look_ahead(1) == TokenKind::Arrow,
            TokenKind::OpenParen => self
                .matching_paren(0)
                .is_some_and(|close| self.look_ahead(close + 1) == TokenKind::Arrow),
            _ => false,
        }
    }

    pub(crate) fn cast_ahead(&self) -> bool {
        if self.token.kind != TokenKind::OpenParen {
            return false;
        }
        let Some(end) = self.scan_type(1) else {
            return false;
        };
        if self.look_ahead(end) != TokenKind::CloseParen {
            return false;
        }

        let next = self.look_ahead(end + 1);
        let is_primitive = end == 2 && is_primitive(self.look_ahead(1));
        if is_primitive {
            starts_operand(next)
                || matches!(
                    next,
                    TokenKind::Minus | TokenKind::PlusPlus | TokenKind::MinusMinus
                )
        } else {
            starts_operand(next)
        }
    }

    pub(crate) fn local_decl_ahead(&self, in_for_header: bool) -> bool {
        let Some(end) = self.scan_type(0) else {
            return false;
        };
        is_plain_ident(self.look_ahead(end))
            && match self.look_ahead(end + 1) {
                TokenKind::Eq | TokenKind::Semi | TokenKind::Comma => true,
                TokenKind::Colon => in_for_header,
                _ => false,
            }
    }

    pub(crate) fn variant_field_ahead(&self, start: usize) -> bool {
        let Some(end) = self.scan_type(start) else {
            return false;
        };
        is_plain_ident(self.look_ahead(end))
            && matches!(
                self.look_ahead(end + 1),
                TokenKind::Comma | TokenKind::CloseParen
            )
    }

    fn matching_paren(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut index = open;
        loop {
            match self.look_ahead(index) {
                TokenKind::OpenParen => depth += 1,
                TokenKind::CloseParen => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                TokenKind::Eof => return None,
                _ => {}
            }
            index += 1;
        }
    }

    fn scan_type(&self, start: usize) -> Option<usize> {
        let mut index = start;
        while let TokenKind::And | TokenKind::AndAnd = self.look_ahead(index) {
            index += 1;
            if self.look_ahead(index) == TokenKind::Ident(kw::Mut) {
                index += 1;
            }
            if self.look_ahead(index) == TokenKind::OpenBracket {
                index = self.scan_borrow_sources(index)?;
            }
        }

        index = match self.look_ahead(index) {
            TokenKind::OpenParen => {
                let inner = self.scan_type(index + 1)?;
                if self.look_ahead(inner) != TokenKind::CloseParen {
                    return None;
                }
                inner + 1
            }
            kind if is_primitive(kind) => index + 1,
            kind if is_plain_ident(kind) => {
                let mut end = index + 1;
                while self.look_ahead(end) == TokenKind::Dot
                    && is_plain_ident(self.look_ahead(end + 1))
                {
                    end += 2;
                }
                if self.look_ahead(end) == TokenKind::Lt {
                    end = self.scan_generic_args(end)?;
                }
                end
            }
            _ => return None,
        };

        loop {
            match (self.look_ahead(index), self.look_ahead(index + 1)) {
                (TokenKind::OpenBracket, TokenKind::CloseBracket) => index += 2,
                (TokenKind::Question, _) => index += 1,
                _ => return Some(index),
            }
        }
    }

    fn scan_borrow_sources(&self, open: usize) -> Option<usize> {
        let mut index = open + 1;
        loop {
            match self.look_ahead(index) {
                TokenKind::CloseBracket => return Some(index + 1),
                kind if is_plain_ident(kind)
                    || kind == TokenKind::Comma
                    || kind == TokenKind::Ident(kw::This) =>
                {
                    index += 1
                }
                _ => return None,
            }
        }
    }

    fn scan_generic_args(&self, open: usize) -> Option<usize> {
        let mut depth = 0i32;
        let mut index = open;
        loop {
            depth += match self.look_ahead(index) {
                TokenKind::Lt => 1,
                TokenKind::Gt => -1,
                TokenKind::Shr => -2,
                TokenKind::UShr => -3,
                TokenKind::Ident(_)
                | TokenKind::Dot
                | TokenKind::Comma
                | TokenKind::Question
                | TokenKind::And
                | TokenKind::AndAnd
                | TokenKind::OpenBracket
                | TokenKind::CloseBracket => 0,
                _ => return None,
            };
            index += 1;
            if depth == 0 {
                return Some(index);
            }
            if depth < 0 {
                return None;
            }
        }
    }
}

pub(crate) fn is_plain_ident(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Ident(symbol) if !symbol.is_keyword())
}

fn is_primitive(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Ident(symbol) if PrimitiveTy::from_symbol(symbol).is_some())
}

fn starts_operand(kind: TokenKind) -> bool {
    match kind {
        TokenKind::Literal(_) | TokenKind::OpenParen | TokenKind::Bang | TokenKind::Tilde => true,
        TokenKind::Ident(symbol) => {
            !symbol.is_keyword()
                || matches!(
                    symbol,
                    kw::This | kw::Super | kw::New | kw::True | kw::False | kw::Null
                )
        }
        _ => false,
    }
}

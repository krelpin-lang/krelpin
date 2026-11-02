use krelpin_ast::ast::{CaseLabel, Pat, PatKind};
use krelpin_ast::token::TokenKind;
use krelpin_errors::Diagnostic;
use krelpin_span::kw;

use super::lookahead::is_plain_ident;
use super::{PResult, Parser};

impl Parser<'_> {
    pub(crate) fn parse_case_label(&mut self) -> PResult<CaseLabel> {
        let label = if self.eat_keyword(kw::Default) {
            CaseLabel::Default(self.prev_token.span)
        } else if self.eat_keyword(kw::Case) {
            let mut pats = vec![self.parse_pat()?];
            while self.eat(TokenKind::Comma) {
                pats.push(self.parse_pat()?);
            }
            let guard = if self.eat_keyword(kw::When) {
                Some(self.parse_ternary()?)
            } else {
                None
            };
            CaseLabel::Case { pats, guard }
        } else {
            return Err(self.unexpected());
        };

        if self.token.kind == TokenKind::Colon {
            return Err(self.emit(
                Diagnostic::error("switch cases must use `->`")
                    .with_span(self.token.span)
                    .with_note("switch cases in Krelpin never fall through to the next case")
                    .with_help("write the case as `case 1 -> ...`"),
            ));
        }
        self.expect(TokenKind::Arrow)?;
        Ok(label)
    }

    pub(crate) fn parse_instanceof_pat(&mut self) -> PResult<Pat> {
        let pat = self.parse_pat()?;
        if matches!(pat.kind, PatKind::Wild | PatKind::Lit(_)) {
            self.emit(
                Diagnostic::error("`instanceof` needs a type or a variant")
                    .with_span(pat.span)
                    .with_help("to compare with a value, use `==`"),
            );
        }
        Ok(pat)
    }

    pub(crate) fn parse_pat(&mut self) -> PResult<Pat> {
        let lo = self.token.span;
        let kind = if self.eat_keyword(kw::Underscore) {
            PatKind::Wild
        } else if is_plain_ident(self.token.kind) && self.path_pat_ahead() {
            let path = self.parse_path()?;
            if self.token.kind == TokenKind::OpenParen {
                let fields = self.parse_field_pats()?;
                PatKind::Variant { path, fields }
            } else {
                PatKind::Path(path)
            }
        } else if self.ty_pat_ahead() {
            let ty = self.parse_ty()?;
            let binding = if self.eat_keyword(kw::Underscore) {
                None
            } else if is_plain_ident(self.token.kind) {
                Some(self.parse_ident()?)
            } else {
                None
            };
            PatKind::Ty { ty, binding }
        } else {
            PatKind::Lit(self.parse_ternary()?)
        };

        Ok(Pat {
            kind,
            span: lo.to(self.prev_token.span),
        })
    }

    fn parse_field_pats(&mut self) -> PResult<Vec<Pat>> {
        self.expect(TokenKind::OpenParen)?;
        let mut fields = Vec::new();
        if self.eat(TokenKind::CloseParen) {
            return Ok(fields);
        }
        loop {
            fields.push(self.parse_pat()?);
            if self.eat(TokenKind::CloseParen) {
                return Ok(fields);
            }
            self.expect(TokenKind::Comma)?;
        }
    }

    fn path_pat_ahead(&self) -> bool {
        let mut end = 1;
        while self.look_ahead(end) == TokenKind::Dot && is_plain_ident(self.look_ahead(end + 1)) {
            end += 2;
        }
        !matches!(
            self.look_ahead(end),
            TokenKind::Lt | TokenKind::OpenBracket | TokenKind::Question
        ) && !is_plain_ident(self.look_ahead(end))
            && self.look_ahead(end) != TokenKind::Ident(kw::Underscore)
    }
}

use krelpin_ast::ast::{GenericParam, GenericParamKind, Generics, Path, PrimitiveTy, Ty, TyKind};
use krelpin_ast::token::TokenKind;
use krelpin_span::kw;

use super::{Expected, PResult, Parser};

impl Parser<'_> {
    pub(crate) fn parse_ty(&mut self) -> PResult<Ty> {
        let lo = self.token.span;
        let mut ty = self.parse_ty_without_suffix()?;

        while self.eat_noexpect(TokenKind::Question) {
            ty = Ty {
                kind: TyKind::Nullable(Box::new(ty)),
                span: lo.to(self.prev_token.span),
            };
        }

        Ok(ty)
    }

    fn parse_ty_without_suffix(&mut self) -> PResult<Ty> {
        let lo = self.token.span;

        if self.break_and_eat(TokenKind::And) {
            let mutable = self.eat_keyword_noexpect(kw::Mut);
            let sources = if self.eat_noexpect(TokenKind::OpenBracket) {
                self.parse_borrow_sources()?
            } else {
                Vec::new()
            };
            let ty = Box::new(self.parse_ty_without_suffix()?);
            return Ok(Ty {
                kind: TyKind::Ref {
                    mutable,
                    sources,
                    ty,
                },
                span: lo.to(self.prev_token.span),
            });
        }

        let mut ty = self.parse_ty_atom()?;
        while self.token.kind == TokenKind::OpenBracket
            && self.look_ahead(1) == TokenKind::CloseBracket
        {
            self.bump();
            self.bump();
            ty = Ty {
                kind: TyKind::Array(Box::new(ty)),
                span: lo.to(self.prev_token.span),
            };
        }
        Ok(ty)
    }

    fn parse_ty_atom(&mut self) -> PResult<Ty> {
        let lo = self.token.span;

        if self.eat_noexpect(TokenKind::OpenParen) {
            let ty = self.parse_ty()?;
            self.expect(TokenKind::CloseParen)?;
            return Ok(Ty {
                kind: ty.kind,
                span: lo.to(self.prev_token.span),
            });
        }

        let primitive = match self.token.kind {
            TokenKind::Ident(symbol) => PrimitiveTy::from_symbol(symbol),
            _ => None,
        };
        if let Some(primitive) = primitive {
            self.bump();
            return Ok(Ty {
                kind: TyKind::Primitive(primitive),
                span: lo,
            });
        }

        if self.token.ident().is_none() {
            self.expected.push(Expected::Type);
            return Err(self.unexpected());
        }

        let path = self.parse_path()?;
        let args = self.parse_generic_args()?;
        Ok(Ty {
            kind: TyKind::Path { path, args },
            span: lo.to(self.prev_token.span),
        })
    }

    pub(crate) fn parse_path(&mut self) -> PResult<Path> {
        let mut segments = vec![self.parse_ident()?];
        while self.token.kind == TokenKind::Dot && self.look_ahead(1) != TokenKind::Star {
            self.bump();
            segments.push(self.parse_ident()?);
        }
        Ok(Path::new(segments))
    }

    fn parse_borrow_sources(&mut self) -> PResult<Vec<krelpin_ast::ast::Ident>> {
        let mut sources = vec![self.parse_ident()?];
        while self.eat(TokenKind::Comma) {
            sources.push(self.parse_ident()?);
        }
        self.expect(TokenKind::CloseBracket)?;
        Ok(sources)
    }

    fn parse_generic_args(&mut self) -> PResult<Vec<Ty>> {
        if !self.eat_noexpect(TokenKind::Lt) {
            return Ok(Vec::new());
        }

        let mut args = vec![self.parse_ty()?];
        loop {
            if self.break_and_eat(TokenKind::Gt) {
                return Ok(args);
            }
            if !self.eat(TokenKind::Comma) {
                self.expected.push(Expected::Token(TokenKind::Gt));
                return Err(self.unexpected());
            }
            args.push(self.parse_ty()?);
        }
    }

    pub(crate) fn parse_ty_list(&mut self) -> PResult<Vec<Ty>> {
        let mut tys = vec![self.parse_ty()?];
        while self.eat(TokenKind::Comma) {
            tys.push(self.parse_ty()?);
        }
        Ok(tys)
    }

    pub(crate) fn parse_generics(&mut self) -> PResult<Generics> {
        let lo = self.token.span;
        if !self.eat_noexpect(TokenKind::Lt) {
            return Ok(Generics {
                params: Vec::new(),
                span: lo.shrink_to_lo(),
            });
        }

        let mut params = vec![self.parse_generic_param()?];
        loop {
            if self.break_and_eat(TokenKind::Gt) {
                break;
            }
            if !self.eat(TokenKind::Comma) {
                self.expected.push(Expected::Token(TokenKind::Gt));
                return Err(self.unexpected());
            }
            params.push(self.parse_generic_param()?);
        }

        Ok(Generics {
            params,
            span: lo.to(self.prev_token.span),
        })
    }

    fn parse_generic_param(&mut self) -> PResult<GenericParam> {
        let lo = self.token.span;

        if self.break_and_eat(TokenKind::And) {
            let name = self.parse_ident()?;
            return Ok(GenericParam {
                name,
                kind: GenericParamKind::BorrowSource,
                span: lo.to(self.prev_token.span),
            });
        }

        let name = self.parse_ident()?;
        let mut bounds = Vec::new();
        if self.eat_keyword(kw::Extends) {
            bounds.push(self.parse_ty()?);
            while self.break_and_eat(TokenKind::And) {
                bounds.push(self.parse_ty()?);
            }
        }

        Ok(GenericParam {
            name,
            kind: GenericParamKind::Type { bounds },
            span: lo.to(self.prev_token.span),
        })
    }
}

use krelpin_ast::ast::{
    Constructor, Field, FieldStorage, FnRetTy, Generics, Ident, Method, Param, Receiver, Ty,
    Unparsed, WherePredicate,
};
use krelpin_ast::token::TokenKind;
use krelpin_errors::Diagnostic;
use krelpin_span::kw;

use super::{PResult, Parser};

impl Parser<'_> {
    pub(crate) fn missing_return_type(&mut self) -> super::ErrorReported {
        let span = self.token.span;
        self.emit(
            Diagnostic::error("missing return type for method")
                .with_span(span)
                .with_label("add a return type before the method name")
                .with_help("use `void` if the method does not return a value"),
        )
    }

    pub(crate) fn parse_ret_ty(&mut self) -> PResult<FnRetTy> {
        if self.eat_keyword_noexpect(kw::Void) {
            return Ok(FnRetTy::Void(self.prev_token.span));
        }
        self.parse_ty().map(FnRetTy::Ty)
    }

    pub(crate) fn parse_field(
        &mut self,
        storage: FieldStorage,
        ty: Ty,
        name: Ident,
    ) -> PResult<Field> {
        let init = if self.eat(TokenKind::Eq) {
            Some(self.parse_initializer()?)
        } else {
            None
        };
        self.expect_semi()?;

        Ok(Field {
            storage,
            ty,
            name,
            init,
        })
    }

    pub(crate) fn parse_constructor(&mut self) -> PResult<Constructor> {
        let name = self.parse_ident()?;
        let params = self.parse_params()?;
        let throws = self.parse_throws()?;
        let body = self.parse_block()?;

        Ok(Constructor {
            name,
            params,
            throws,
            body,
        })
    }

    pub(crate) fn parse_method(
        &mut self,
        generics: Generics,
        ret: FnRetTy,
        name: Ident,
        receiver: Option<Receiver>,
    ) -> PResult<Method> {
        let params = self.parse_params()?;
        let throws = self.parse_throws()?;
        let where_clause = self.parse_where_clause()?;
        let body = if self.eat(TokenKind::Semi) {
            None
        } else {
            Some(self.parse_block()?)
        };

        Ok(Method {
            generics,
            ret,
            name,
            receiver,
            params,
            throws,
            where_clause,
            body,
        })
    }

    fn parse_where_clause(&mut self) -> PResult<Vec<WherePredicate>> {
        let mut predicates = Vec::new();
        if !self.eat_keyword(kw::Where) {
            return Ok(predicates);
        }
        loop {
            let lo = self.token.span;
            let param = self.parse_ident()?;
            if !self.eat_keyword(kw::Extends) {
                return Err(self.unexpected());
            }
            let bounds = self.parse_bounds()?;
            predicates.push(WherePredicate {
                param,
                bounds,
                span: lo.to(self.prev_token.span),
            });
            if !self.eat(TokenKind::Comma) {
                return Ok(predicates);
            }
        }
    }

    fn parse_throws(&mut self) -> PResult<Vec<Ty>> {
        if self.eat_keyword(kw::Throws) {
            self.parse_ty_list()
        } else {
            Ok(Vec::new())
        }
    }

    fn parse_params(&mut self) -> PResult<Vec<Param>> {
        self.expect(TokenKind::OpenParen)?;
        let mut params = Vec::new();
        if self.eat(TokenKind::CloseParen) {
            return Ok(params);
        }

        loop {
            if !self.reject_receiver_param() {
                params.push(self.parse_param()?);
            }
            if self.eat(TokenKind::CloseParen) {
                return Ok(params);
            }
            self.expect(TokenKind::Comma)?;
        }
    }

    fn reject_receiver_param(&mut self) -> bool {
        let lo = self.token.span;
        let (written, help, len) =
            match (self.look_ahead(0), self.look_ahead(1), self.look_ahead(2)) {
                (TokenKind::Ident(kw::This), _, _) => {
                    ("this", "remove it and mark the method `consume`", 1)
                }
                (TokenKind::And, TokenKind::Ident(kw::This), _) => (
                    "&this",
                    "remove it; methods read `this` without changing it by default",
                    2,
                ),
                (TokenKind::And, TokenKind::Ident(kw::Mut), TokenKind::Ident(kw::This)) => {
                    ("&mut this", "remove it and mark the method `mut`", 3)
                }
                _ => return false,
            };
        for _ in 0..len {
            self.bump();
        }
        self.emit(
            Diagnostic::error(format!("`{written}` is not written as a parameter"))
                .with_span(lo.to(self.prev_token.span))
                .with_note("every method that is not `static` can use `this`")
                .with_help(help),
        );
        true
    }

    fn parse_param(&mut self) -> PResult<Param> {
        let lo = self.token.span;
        let ty = self.parse_ty()?;
        let variadic = self.eat_noexpect(TokenKind::DotDotDot);
        let name = self.parse_ident()?;

        Ok(Param {
            ty,
            name,
            variadic,
            span: lo.to(self.prev_token.span),
        })
    }

    pub(crate) fn parse_unparsed_delimited(
        &mut self,
        open: TokenKind,
        close: TokenKind,
    ) -> PResult<Unparsed> {
        let open_span = self.expect(open)?;
        let mut depth = 1usize;
        let mut tokens = Vec::new();

        loop {
            match self.token.kind {
                TokenKind::Eof => return Err(self.unclosed_delimiter(open_span)),
                kind if kind == open => depth += 1,
                kind if kind == close => {
                    depth -= 1;
                    if depth == 0 {
                        self.bump();
                        return Ok(Unparsed {
                            tokens,
                            span: open_span.to(self.prev_token.span),
                        });
                    }
                }
                _ => {}
            }
            tokens.push(self.token);
            self.bump();
        }
    }
}

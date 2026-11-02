use krelpin_ast::ast::{
    Constructor, Field, FieldStorage, FnRetTy, Generics, Ident, Method, Param, Receiver,
    ReceiverKind, Ty, Unparsed, WherePredicate,
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
        let (receiver, params) = self.parse_params()?;
        if let Some(receiver) = receiver {
            self.emit(
                Diagnostic::error("constructors cannot take `this` as a parameter")
                    .with_span(receiver.span)
                    .with_help("constructors always have access to the object being built"),
            );
        }
        let throws = self.parse_throws()?;
        let body = self.parse_block()?;

        Ok(Constructor {
            compact: false,
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
        is_static: bool,
    ) -> PResult<Method> {
        let (receiver, params) = self.parse_params()?;
        let receiver = self.check_static(is_static, receiver, name);
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

    pub(crate) fn parse_params(&mut self) -> PResult<(Option<Receiver>, Vec<Param>)> {
        self.expect(TokenKind::OpenParen)?;
        let mut receiver = None;
        let mut params = Vec::new();

        if self.eat(TokenKind::CloseParen) {
            return Ok((receiver, params));
        }

        loop {
            match self.parse_receiver() {
                Some(found) if receiver.is_some() => {
                    self.error(found.span, "`this` can only be taken once");
                }
                Some(found) => {
                    if !params.is_empty() {
                        self.emit(
                            Diagnostic::error("`this` must be the first parameter")
                                .with_span(found.span)
                                .with_help("move `this` to the start of the parameter list"),
                        );
                    }
                    receiver = Some(found);
                }
                None => params.push(self.parse_param()?),
            }

            if self.eat(TokenKind::CloseParen) {
                return Ok((receiver, params));
            }
            self.expect(TokenKind::Comma)?;
        }
    }

    fn parse_receiver(&mut self) -> Option<Receiver> {
        let lo = self.token.span;
        let (kind, len) = match (self.look_ahead(0), self.look_ahead(1), self.look_ahead(2)) {
            (TokenKind::Ident(kw::This), _, _) => (ReceiverKind::Owned, 1),
            (TokenKind::And, TokenKind::Ident(kw::This), _) => (ReceiverKind::Ref, 2),
            (TokenKind::And, TokenKind::Ident(kw::Mut), TokenKind::Ident(kw::This)) => {
                (ReceiverKind::RefMut, 3)
            }
            _ => return None,
        };

        for _ in 0..len {
            self.bump();
        }
        Some(Receiver {
            kind,
            span: lo.to(self.prev_token.span),
        })
    }

    fn check_static(
        &self,
        is_static: bool,
        receiver: Option<Receiver>,
        name: Ident,
    ) -> Option<Receiver> {
        match (is_static, receiver) {
            (true, Some(receiver)) => {
                self.emit(
                    Diagnostic::error("a `static` method cannot take `this`")
                        .with_span(receiver.span)
                        .with_help("remove `static`, or remove the `this` parameter"),
                );
                None
            }
            (false, None) => {
                self.emit(
                    Diagnostic::error(format!(
                        "the method `{}` has no `this` and must be `static`",
                        name.name
                    ))
                    .with_span(name.span)
                    .with_help("mark the method `static`, or add `&this` as its first parameter"),
                );
                None
            }
            (_, receiver) => receiver,
        }
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

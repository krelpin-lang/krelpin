use krelpin_ast::ast::{
    Annotation, Class, CompilationUnit, Constructor, Field, FieldStorage, FnRetTy, Generics, Ident,
    Import, Item, ItemKind, Member, MemberKind, Method, Param, Receiver, ReceiverKind, Unparsed,
    Visibility,
};
use krelpin_ast::token::TokenKind;
use krelpin_errors::Diagnostic;
use krelpin_span::{Symbol, kw};

use super::{Expected, PResult, Parser};

struct Modifiers {
    vis: Visibility,
    storage: FieldStorage,
}

impl Parser<'_> {
    pub(crate) fn parse_compilation_unit(&mut self) -> CompilationUnit {
        let lo = self.token.span;
        let mut docs = self.parse_docs();
        let mut unit_docs = Vec::new();

        let mut package = None;
        if self.token.is_keyword(kw::Package) {
            unit_docs = std::mem::take(&mut docs);
            match self.parse_package() {
                Ok(path) => package = Some(path),
                Err(_) => self.recover_to_member_end(),
            }
            docs = self.parse_docs();
        }

        let mut imports = Vec::new();
        while self.token.is_keyword(kw::Import) {
            match self.parse_import() {
                Ok(import) => imports.push(import),
                Err(_) => self.recover_to_member_end(),
            }
            docs = self.parse_docs();
        }

        let mut items = Vec::new();
        loop {
            docs.extend(self.parse_docs());
            if self.token.kind == TokenKind::Eof {
                break;
            }
            match self.parse_item(std::mem::take(&mut docs)) {
                Ok(item) => items.push(item),
                Err(_) => self.recover_to_next_item(),
            }
        }

        CompilationUnit {
            docs: unit_docs,
            package,
            imports,
            items,
            span: lo.to(self.token.span),
        }
    }

    fn parse_package(&mut self) -> PResult<krelpin_ast::ast::Path> {
        self.bump();
        let path = self.parse_path()?;
        self.expect(TokenKind::Semi)?;
        Ok(path)
    }

    fn parse_import(&mut self) -> PResult<Import> {
        let lo = self.token.span;
        self.bump();

        let path = self.parse_path()?;
        let glob = self.token.kind == TokenKind::Dot && self.look_ahead(1) == TokenKind::Star;
        if glob {
            self.bump();
            self.bump();
        }
        self.expect(TokenKind::Semi)?;

        Ok(Import {
            path,
            glob,
            span: lo.to(self.prev_token.span),
        })
    }

    fn parse_item(&mut self, docs: Vec<Symbol>) -> PResult<Item> {
        let lo = self.token.span;
        let annotations = self.parse_annotations()?;
        let modifiers = self.parse_modifiers();
        self.reject_storage(modifiers.storage, "classes");

        if !self.token.is_keyword(kw::Class) {
            self.expected = vec![Expected::Item];
            return Err(self.unexpected());
        }
        self.bump();

        let name = self.parse_ident()?;
        let generics = self.parse_generics()?;
        let extends = if self.eat_keyword(kw::Extends) {
            Some(self.parse_ty()?)
        } else {
            None
        };
        let implements = if self.eat_keyword(kw::Implements) {
            self.parse_ty_list()?
        } else {
            Vec::new()
        };
        let members = self.parse_class_body(name)?;

        Ok(Item {
            docs,
            annotations,
            vis: modifiers.vis,
            name,
            kind: ItemKind::Class(Class {
                generics,
                extends,
                implements,
                members,
            }),
            span: lo.to(self.prev_token.span),
        })
    }

    fn parse_class_body(&mut self, class_name: Ident) -> PResult<Vec<Member>> {
        let open = self.expect(TokenKind::OpenBrace)?;
        let mut members = Vec::new();

        loop {
            let docs = self.parse_docs();
            if self.eat_noexpect(TokenKind::CloseBrace) {
                return Ok(members);
            }
            if self.eat_noexpect(TokenKind::Semi) {
                continue;
            }
            if self.token.kind == TokenKind::Eof {
                return Err(self.unclosed_delimiter(open));
            }

            match self.parse_member(docs, class_name) {
                Ok(member) => members.push(member),
                Err(error) => {
                    self.recover_to_member_end();
                    if self.token.kind == TokenKind::Eof {
                        return Err(error);
                    }
                }
            }
        }
    }

    fn parse_annotations(&mut self) -> PResult<Vec<Annotation>> {
        let mut annotations = Vec::new();
        while self.token.kind == TokenKind::At {
            let lo = self.token.span;
            self.bump();
            let path = self.parse_path()?;
            let args = if self.token.kind == TokenKind::OpenParen {
                Some(self.parse_unparsed_delimited(TokenKind::OpenParen, TokenKind::CloseParen)?)
            } else {
                None
            };
            annotations.push(Annotation {
                path,
                args,
                span: lo.to(self.prev_token.span),
            });
        }
        Ok(annotations)
    }

    fn parse_modifiers(&mut self) -> Modifiers {
        let mut modifiers = Modifiers {
            vis: Visibility::Default,
            storage: FieldStorage::Instance,
        };

        loop {
            let span = self.token.span;
            let TokenKind::Ident(symbol) = self.token.kind else {
                return modifiers;
            };

            let vis = match symbol {
                kw::Public => Some(Visibility::Public(span)),
                kw::Protected => Some(Visibility::Protected(span)),
                kw::Private => Some(Visibility::Private(span)),
                _ => None,
            };
            let storage = match symbol {
                kw::Const => Some(FieldStorage::Const(span)),
                kw::Shared => Some(FieldStorage::Shared(span)),
                _ => None,
            };

            match (vis, storage) {
                (Some(vis), _) if modifiers.vis == Visibility::Default => modifiers.vis = vis,
                (Some(_), _) => {
                    self.emit(
                        Diagnostic::error("multiple visibility modifiers")
                            .with_span(span)
                            .with_help("remove all but one visibility modifier"),
                    );
                }
                (_, Some(storage)) if modifiers.storage == FieldStorage::Instance => {
                    modifiers.storage = storage;
                }
                (_, Some(_)) => {
                    self.emit(
                        Diagnostic::error("a field can only have one of `const` and `shared`")
                            .with_span(span),
                    );
                }
                (None, None) => return modifiers,
            }
            self.bump();
        }
    }

    fn reject_storage(&self, storage: FieldStorage, what: &str) {
        let (keyword, span) = match storage {
            FieldStorage::Instance => return,
            FieldStorage::Const(span) => ("const", span),
            FieldStorage::Shared(span) => ("shared", span),
        };
        self.emit(
            Diagnostic::error(format!("`{keyword}` cannot be used on {what}"))
                .with_span(span)
                .with_help(format!("`{keyword}` is only allowed on fields")),
        );
    }

    fn parse_member(&mut self, docs: Vec<Symbol>, class_name: Ident) -> PResult<Member> {
        let lo = self.token.span;
        let annotations = self.parse_annotations()?;
        let modifiers = self.parse_modifiers();
        let kind = self.parse_member_kind(class_name, modifiers.storage)?;

        Ok(Member {
            docs,
            annotations,
            vis: modifiers.vis,
            kind,
            span: lo.to(self.prev_token.span),
        })
    }

    fn parse_member_kind(
        &mut self,
        class_name: Ident,
        storage: FieldStorage,
    ) -> PResult<MemberKind> {
        if self.token.kind == TokenKind::Lt {
            self.reject_storage(storage, "methods");
            let generics = self.parse_generics()?;
            let ret = self.parse_ret_ty()?;
            let name = self.parse_ident()?;
            return self
                .parse_method(generics, ret, name)
                .map(MemberKind::Method);
        }

        let called = match self.token.ident() {
            Some(name) if self.look_ahead(1) == TokenKind::OpenParen => Some(name),
            _ => None,
        };
        if let Some(name) = called {
            if name != class_name.name {
                return Err(self.missing_return_type());
            }
            self.reject_storage(storage, "constructors");
            return self.parse_constructor().map(MemberKind::Constructor);
        }

        let ret = self.parse_ret_ty()?;
        let name = self.parse_ident()?;

        if self.check(TokenKind::OpenParen) {
            self.reject_storage(storage, "methods");
            return self
                .parse_method(Generics::default(), ret, name)
                .map(MemberKind::Method);
        }

        let ty = match ret {
            FnRetTy::Ty(ty) => ty,
            FnRetTy::Void(span) => {
                return Err(self.error(span, "fields cannot have type `void`"));
            }
        };
        self.parse_field(storage, ty, name).map(MemberKind::Field)
    }

    fn missing_return_type(&mut self) -> super::ErrorReported {
        let span = self.token.span;
        self.emit(
            Diagnostic::error("missing return type for method")
                .with_span(span)
                .with_label("add a return type before the method name")
                .with_help("use `void` if the method does not return a value"),
        )
    }

    fn parse_ret_ty(&mut self) -> PResult<FnRetTy> {
        if self.eat_keyword_noexpect(kw::Void) {
            return Ok(FnRetTy::Void(self.prev_token.span));
        }
        self.parse_ty().map(FnRetTy::Ty)
    }

    fn parse_field(
        &mut self,
        storage: FieldStorage,
        ty: krelpin_ast::ast::Ty,
        name: Ident,
    ) -> PResult<Field> {
        let init = if self.eat(TokenKind::Eq) {
            Some(self.parse_unparsed_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::Semi)?;

        Ok(Field {
            storage,
            ty,
            name,
            init,
        })
    }

    fn parse_constructor(&mut self) -> PResult<Constructor> {
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
        let body = self.parse_unparsed_delimited(TokenKind::OpenBrace, TokenKind::CloseBrace)?;

        Ok(Constructor {
            name,
            params,
            throws,
            body,
        })
    }

    fn parse_method(&mut self, generics: Generics, ret: FnRetTy, name: Ident) -> PResult<Method> {
        let (receiver, params) = self.parse_params()?;
        let throws = self.parse_throws()?;
        let body = if self.eat(TokenKind::Semi) {
            None
        } else {
            Some(self.parse_unparsed_delimited(TokenKind::OpenBrace, TokenKind::CloseBrace)?)
        };

        Ok(Method {
            generics,
            ret,
            name,
            receiver,
            params,
            throws,
            body,
        })
    }

    fn parse_throws(&mut self) -> PResult<Vec<krelpin_ast::ast::Ty>> {
        if self.eat_keyword(kw::Throws) {
            self.parse_ty_list()
        } else {
            Ok(Vec::new())
        }
    }

    fn parse_params(&mut self) -> PResult<(Option<Receiver>, Vec<Param>)> {
        self.expect(TokenKind::OpenParen)?;
        let mut receiver = None;
        let mut params = Vec::new();

        if self.eat(TokenKind::CloseParen) {
            return Ok((receiver, params));
        }

        loop {
            match self.parse_receiver() {
                Some(found) if receiver.is_none() && params.is_empty() => receiver = Some(found),
                Some(found) => {
                    self.emit(
                        Diagnostic::error("`this` must be the first parameter")
                            .with_span(found.span)
                            .with_help("move `this` to the start of the parameter list"),
                    );
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

    fn parse_unparsed_delimited(&mut self, open: TokenKind, close: TokenKind) -> PResult<Unparsed> {
        let open_span = self.expect(open)?;
        let mut depth = 1usize;

        loop {
            match self.token.kind {
                TokenKind::Eof => return Err(self.unclosed_delimiter(open_span)),
                kind if kind == open => depth += 1,
                kind if kind == close => {
                    depth -= 1;
                    if depth == 0 {
                        self.bump();
                        return Ok(Unparsed {
                            span: open_span.to(self.prev_token.span),
                        });
                    }
                }
                _ => {}
            }
            self.bump();
        }
    }

    fn parse_unparsed_expr(&mut self) -> PResult<Unparsed> {
        let lo = self.token.span;
        if matches!(self.token.kind, TokenKind::Semi | TokenKind::CloseBrace) {
            self.expected.push(Expected::Expr);
            return Err(self.unexpected());
        }

        let mut depth = 0usize;
        loop {
            match self.token.kind {
                TokenKind::Eof => break,
                TokenKind::Semi | TokenKind::CloseBrace if depth == 0 => break,
                TokenKind::OpenParen | TokenKind::OpenBrace | TokenKind::OpenBracket => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseBrace | TokenKind::CloseBracket => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
            self.bump();
        }

        Ok(Unparsed {
            span: lo.to(self.prev_token.span),
        })
    }
}

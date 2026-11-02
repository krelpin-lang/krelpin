use krelpin_ast::ast::{
    Annotation, Class, CompilationUnit, Enum, FieldStorage, FnRetTy, Generics, Ident, Import,
    Inheritance, Interface, Item, ItemKind, MacroDef, Member, MemberKind, Path, Struct, Ty,
    Variant, VariantField, Visibility, is_intrinsic,
};
use krelpin_ast::token::TokenKind;
use krelpin_errors::Diagnostic;
use krelpin_span::{Span, Symbol, kw};

use super::lookahead::is_plain_ident;
use super::{Expected, PResult, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Container {
    Class,
    Struct,
    Interface,
    Enum,
}

impl Container {
    fn plural(self) -> &'static str {
        match self {
            Container::Class => "classes",
            Container::Struct => "structs",
            Container::Interface => "interfaces",
            Container::Enum => "enums",
        }
    }
}

enum Declaration {
    Item(Item),
    Macro(MacroDef),
}

struct Modifiers {
    vis: Visibility,
    storage: FieldStorage,
    inheritance: Inheritance,
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
        let mut macros = Vec::new();
        loop {
            docs.extend(self.parse_docs());
            if self.token.kind == TokenKind::Eof {
                break;
            }
            match self.parse_declaration(std::mem::take(&mut docs)) {
                Ok(Declaration::Item(item)) => items.push(item),
                Ok(Declaration::Macro(def)) => macros.push(def),
                Err(_) => self.recover_to_next_item(),
            }
        }

        CompilationUnit {
            docs: unit_docs,
            package,
            imports,
            items,
            macros,
            span: lo.to(self.token.span),
        }
    }

    fn parse_package(&mut self) -> PResult<Path> {
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

    fn parse_declaration(&mut self, docs: Vec<Symbol>) -> PResult<Declaration> {
        let lo = self.token.span;
        let annotations = self.parse_annotations()?;
        let modifiers = self.parse_modifiers();
        if self.token.is_keyword(kw::Macro) {
            return self
                .parse_macro(docs, annotations, &modifiers, lo)
                .map(Declaration::Macro);
        }
        self.reject_storage(modifiers.storage, "type declarations");

        let container = match self.token.kind {
            TokenKind::Ident(kw::Class) => Container::Class,
            TokenKind::Ident(kw::Struct) => Container::Struct,
            TokenKind::Ident(kw::Interface) => Container::Interface,
            TokenKind::Ident(kw::Enum) => Container::Enum,
            _ => {
                self.expected = vec![Expected::Item];
                return Err(self.unexpected());
            }
        };
        self.bump();

        if container != Container::Class {
            self.reject_inheritance(modifiers.inheritance, container.plural());
        } else if let Inheritance {
            abstract_kw: Some(_),
            final_kw: Some(span),
        } = modifiers.inheritance
        {
            self.emit(
                Diagnostic::error("a class cannot be both `abstract` and `final`")
                    .with_span(span)
                    .with_note("an abstract class must be extended, a final class cannot be"),
            );
        }

        let name = self.parse_ident()?;
        let kind = match container {
            Container::Class => self.parse_class(name)?,
            Container::Struct => self.parse_struct(name)?,
            Container::Interface => self.parse_interface(name)?,
            Container::Enum => self.parse_enum(name)?,
        };

        Ok(Declaration::Item(Item {
            docs,
            annotations,
            vis: modifiers.vis,
            inheritance: modifiers.inheritance,
            name,
            kind,
            span: lo.to(self.prev_token.span),
        }))
    }

    fn parse_macro(
        &mut self,
        docs: Vec<Symbol>,
        annotations: Vec<Annotation>,
        modifiers: &Modifiers,
        lo: Span,
    ) -> PResult<MacroDef> {
        self.bump();
        self.reject_storage(modifiers.storage, "macros");
        self.reject_inheritance(modifiers.inheritance, "macros");
        if let Some(span) = modifiers.vis.span() {
            self.error(span, "macros cannot have a visibility modifier");
        }
        let name = self.parse_ident()?;
        if self.token.kind == TokenKind::OpenBrace {
            let body =
                self.parse_unparsed_delimited(TokenKind::OpenBrace, TokenKind::CloseBrace)?;
            self.emit(
                Diagnostic::error("macros written in Krelpin are not supported yet")
                    .with_span(body.span)
                    .with_note("only the macros of the standard library can be used for now"),
            );
        } else {
            self.expect_semi()?;
        }
        Ok(MacroDef {
            docs,
            annotations,
            name,
            span: lo.to(self.prev_token.span),
        })
    }

    fn parse_class(&mut self, name: Ident) -> PResult<ItemKind> {
        let generics = self.parse_generics()?;
        let extends = if self.eat_keyword(kw::Extends) {
            Some(self.parse_ty()?)
        } else {
            None
        };
        let implements = self.parse_implements()?;
        let members = self.parse_body(Container::Class, name)?;

        Ok(ItemKind::Class(Class {
            generics,
            extends,
            implements,
            members,
        }))
    }

    fn parse_struct(&mut self, name: Ident) -> PResult<ItemKind> {
        let generics = self.parse_generics()?;
        self.reject_extends("structs")?;
        let implements = self.parse_implements()?;
        let members = self.parse_body(Container::Struct, name)?;

        Ok(ItemKind::Struct(Struct {
            generics,
            implements,
            members,
        }))
    }

    fn parse_interface(&mut self, name: Ident) -> PResult<ItemKind> {
        let generics = self.parse_generics()?;
        let extends = if self.eat_keyword(kw::Extends) {
            self.parse_ty_list()?
        } else {
            Vec::new()
        };

        if self.token.is_keyword(kw::Implements) {
            self.emit(
                Diagnostic::error("interfaces cannot implement other interfaces")
                    .with_span(self.token.span)
                    .with_help("use `extends` to inherit from other interfaces"),
            );
            self.bump();
            self.parse_ty_list()?;
        }

        let members = self.parse_body(Container::Interface, name)?;
        Ok(ItemKind::Interface(Interface {
            generics,
            extends,
            members,
        }))
    }

    fn parse_enum(&mut self, name: Ident) -> PResult<ItemKind> {
        let generics = self.parse_generics()?;
        self.reject_extends("enums")?;
        let implements = self.parse_implements()?;

        let open = self.expect(TokenKind::OpenBrace)?;
        let mut variants = Vec::new();
        while !matches!(self.token.kind, TokenKind::Semi | TokenKind::CloseBrace) {
            variants.push(self.parse_variant(name)?);
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }

        let members = if self.eat(TokenKind::Semi) {
            self.parse_member_list(open, Container::Enum, name)?
        } else if self.eat(TokenKind::CloseBrace) {
            Vec::new()
        } else {
            return Err(self.unexpected());
        };

        Ok(ItemKind::Enum(Enum {
            generics,
            implements,
            variants,
            members,
        }))
    }

    fn parse_variant(&mut self, owner: Ident) -> PResult<Variant> {
        let lo = self.token.span;
        let docs = self.parse_docs();
        let annotations = self.parse_annotations()?;
        if !is_plain_ident(self.token.kind) {
            self.expected.push(Expected::Ident);
            return Err(self.unexpected());
        }
        let name = self.parse_ident()?;
        let fields = if self.token.kind == TokenKind::OpenParen {
            self.parse_variant_fields(owner, name)?
        } else {
            Vec::new()
        };

        Ok(Variant {
            docs,
            annotations,
            name,
            fields,
            span: lo.to(self.prev_token.span),
        })
    }

    fn parse_variant_fields(&mut self, owner: Ident, variant: Ident) -> PResult<Vec<VariantField>> {
        let open = self.token.span;
        if self.look_ahead(1) == TokenKind::CloseParen {
            self.bump();
            self.bump();
            self.emit(
                Diagnostic::error(format!("the variant `{}` has no fields", variant.name))
                    .with_span(open.to(self.prev_token.span))
                    .with_help("a variant without data is written without parentheses"),
            );
            return Ok(Vec::new());
        }
        if !self.variant_field_ahead(1) {
            self.parse_args()?;
            self.emit(
                Diagnostic::error("enum variants declare fields, not arguments")
                    .with_span(open.to(self.prev_token.span))
                    .with_note("enums have no constructors; each variant carries its own data")
                    .with_help(format!(
                        "declare its fields, like `{variant}(Type name)`, and pass the values \
                         when creating it: `{owner}.{variant}(...)`",
                        variant = variant.name,
                        owner = owner.name,
                    )),
            );
            return Ok(Vec::new());
        }

        self.bump();
        let mut fields = Vec::new();
        loop {
            let ty = self.parse_ty()?;
            let name = self.parse_ident()?;
            fields.push(VariantField { ty, name });
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.expect(TokenKind::CloseParen)?;
        Ok(fields)
    }

    fn parse_implements(&mut self) -> PResult<Vec<Ty>> {
        if self.eat_keyword(kw::Implements) {
            self.parse_ty_list()
        } else {
            Ok(Vec::new())
        }
    }

    fn reject_extends(&mut self, what: &str) -> PResult<()> {
        if !self.token.is_keyword(kw::Extends) {
            return Ok(());
        }
        let lo = self.token.span;
        self.bump();
        self.parse_ty()?;
        self.emit(
            Diagnostic::error(format!("{what} cannot extend other types"))
                .with_span(lo.to(self.prev_token.span))
                .with_help(format!("{what} can implement interfaces with `implements`")),
        );
        Ok(())
    }

    fn parse_body(&mut self, container: Container, owner: Ident) -> PResult<Vec<Member>> {
        let open = self.expect(TokenKind::OpenBrace)?;
        self.parse_member_list(open, container, owner)
    }

    fn parse_member_list(
        &mut self,
        open: Span,
        container: Container,
        owner: Ident,
    ) -> PResult<Vec<Member>> {
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

            match self.parse_member(docs, container, owner) {
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

    pub(crate) fn parse_members_to_eof(&mut self, owner: Ident) -> Vec<Member> {
        let mut members = Vec::new();
        loop {
            let docs = self.parse_docs();
            if self.token.kind == TokenKind::Eof {
                return members;
            }
            if self.eat_noexpect(TokenKind::Semi) {
                continue;
            }
            match self.parse_member(docs, Container::Class, owner) {
                Ok(member) => members.push(member),
                Err(_) => {
                    self.recover_to_member_end();
                    self.eat_noexpect(TokenKind::CloseBrace);
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
            inheritance: Inheritance::default(),
        };

        loop {
            let span = self.token.span;
            let TokenKind::Ident(symbol) = self.token.kind else {
                return modifiers;
            };

            match symbol {
                kw::Public | kw::Protected | kw::Private => {
                    let vis = match symbol {
                        kw::Public => Visibility::Public(span),
                        kw::Protected => Visibility::Protected(span),
                        _ => Visibility::Private(span),
                    };
                    if modifiers.vis == Visibility::Default {
                        modifiers.vis = vis;
                    } else {
                        self.emit(
                            Diagnostic::error("multiple visibility modifiers")
                                .with_span(span)
                                .with_help("remove all but one visibility modifier"),
                        );
                    }
                }
                kw::Const | kw::Shared => {
                    let storage = if symbol == kw::Const {
                        FieldStorage::Const(span)
                    } else {
                        FieldStorage::Shared(span)
                    };
                    if modifiers.storage == FieldStorage::Instance {
                        modifiers.storage = storage;
                    } else {
                        self.error(span, "a field can only have one of `const` and `shared`");
                    }
                }
                kw::Abstract | kw::Final => {
                    let slot = if symbol == kw::Abstract {
                        &mut modifiers.inheritance.abstract_kw
                    } else {
                        &mut modifiers.inheritance.final_kw
                    };
                    if slot.is_some() {
                        self.error(span, format!("duplicate `{symbol}` modifier"));
                    } else {
                        *slot = Some(span);
                    }
                }
                _ => return modifiers,
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

    fn reject_inheritance(&self, inheritance: Inheritance, what: &str) {
        let modifiers = [
            ("abstract", inheritance.abstract_kw),
            ("final", inheritance.final_kw),
        ];
        for (keyword, span) in modifiers {
            if let Some(span) = span {
                self.error(span, format!("`{keyword}` cannot be used on {what}"));
            }
        }
    }

    fn parse_member(
        &mut self,
        docs: Vec<Symbol>,
        container: Container,
        owner: Ident,
    ) -> PResult<Member> {
        let lo = self.token.span;
        let annotations = self.parse_annotations()?;
        let modifiers = self.parse_modifiers();
        let kind = self.parse_member_kind(owner, modifiers.storage)?;
        self.validate_member(container, &annotations, &modifiers, &kind);

        Ok(Member {
            docs,
            annotations,
            vis: modifiers.vis,
            inheritance: modifiers.inheritance,
            kind,
            span: lo.to(self.prev_token.span),
        })
    }

    fn parse_member_kind(&mut self, owner: Ident, storage: FieldStorage) -> PResult<MemberKind> {
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
            if name != owner.name {
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

    fn validate_member(
        &self,
        container: Container,
        annotations: &[Annotation],
        modifiers: &Modifiers,
        kind: &MemberKind,
    ) {
        let Inheritance {
            abstract_kw,
            final_kw,
        } = modifiers.inheritance;

        match kind {
            MemberKind::Field(field) => {
                if let Some(span) = abstract_kw {
                    self.error(span, "`abstract` cannot be used on fields");
                }
                if container == Container::Interface
                    && !matches!(field.storage, FieldStorage::Const(_))
                {
                    self.emit(
                        Diagnostic::error("interfaces can only have `const` fields")
                            .with_span(field.name.span)
                            .with_help("make the field `const`, or move it to a class"),
                    );
                }
                if container == Container::Enum && field.storage == FieldStorage::Instance {
                    self.emit(
                        Diagnostic::error("enums cannot have instance fields")
                            .with_span(field.name.span)
                            .with_help("store the data in the variants that carry it"),
                    );
                }
            }
            MemberKind::Constructor(constructor) => {
                match container {
                    Container::Interface => {
                        self.error(constructor.name.span, "interfaces cannot have constructors");
                    }
                    Container::Enum => {
                        self.emit(
                            Diagnostic::error("enums cannot have constructors")
                                .with_span(constructor.name.span)
                                .with_help("declare the fields of each variant in the variant"),
                        );
                    }
                    Container::Struct => {
                        self.emit(
                            Diagnostic::error("structs cannot have constructors")
                                .with_span(constructor.name.span)
                                .with_note(
                                    "a struct is created from its fields in order, \
                                     like `new Point(1, 2)`",
                                )
                                .with_help("give a field an initial value to leave it out"),
                        );
                    }
                    Container::Class => {}
                }
                self.reject_inheritance(modifiers.inheritance, "constructors");
            }
            MemberKind::Method(method) => {
                if let (Some(_), Some(span)) = (abstract_kw, final_kw) {
                    self.error(span, "a method cannot be both `abstract` and `final`");
                }

                let can_be_abstract = matches!(container, Container::Class | Container::Interface);
                match (abstract_kw, &method.body) {
                    (Some(span), _) if !can_be_abstract => {
                        let what = container.plural();
                        self.error(span, format!("abstract methods are not allowed in {what}"));
                    }
                    (Some(span), Some(_)) => {
                        self.error(span, "abstract methods cannot have a body");
                    }
                    (None, None)
                        if container != Container::Interface && !is_intrinsic(annotations) =>
                    {
                        self.emit(
                            Diagnostic::error(format!(
                                "missing body for method `{}`",
                                method.name.name
                            ))
                            .with_span(method.name.span)
                            .with_help("add a body, or mark the method `abstract`"),
                        );
                    }
                    _ => {}
                }
            }
        }
    }
}

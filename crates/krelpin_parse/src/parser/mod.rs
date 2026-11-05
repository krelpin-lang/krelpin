mod diagnostics;
mod expr;
mod interpolation;
mod item;
mod lookahead;
mod member;
mod pat;
mod stmt;
mod ty;

pub use self::item::Container;

#[cfg(test)]
mod tests;

use krelpin_ast::ast::{CompilationUnit, Ident, Item, Member, Ty};
use krelpin_ast::token::{Token, TokenKind};
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_span::{Span, Symbol};

pub fn parse(tokens: Vec<Token>, dcx: &DiagCtxt) -> CompilationUnit {
    Parser::new(tokens, dcx).parse_compilation_unit()
}

pub fn parse_tys(tokens: Vec<Token>, dcx: &DiagCtxt) -> Option<Vec<Ty>> {
    let mut parser = Parser::new(tokens, dcx);
    let tys = parser.parse_ty_list().ok()?;
    parser.expect(TokenKind::Eof).ok()?;
    Some(tys)
}

pub fn parse_members(tokens: Vec<Token>, owner: Ident, dcx: &DiagCtxt) -> Vec<Member> {
    Parser::new(tokens, dcx).parse_members_to_eof(Container::Class, owner)
}

pub fn parse_members_of(
    tokens: Vec<Token>,
    container: Container,
    owner: Ident,
    dcx: &DiagCtxt,
) -> Vec<Member> {
    Parser::new(tokens, dcx).parse_members_to_eof(container, owner)
}

pub fn parse_items(tokens: Vec<Token>, dcx: &DiagCtxt) -> Vec<Item> {
    Parser::new(tokens, dcx).parse_items_to_eof()
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ErrorReported;

pub(crate) type PResult<T> = Result<T, ErrorReported>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Expected {
    Token(TokenKind),
    Keyword(Symbol),
    Ident,
    Type,
    Expr,
    Item,
}

pub(crate) struct Parser<'a> {
    tokens: Vec<Token>,
    index: usize,
    token: Token,
    prev_token: Token,
    expected: Vec<Expected>,
    dcx: &'a DiagCtxt,
}

impl<'a> Parser<'a> {
    fn new(mut tokens: Vec<Token>, dcx: &'a DiagCtxt) -> Parser<'a> {
        if tokens.last().map(|token| token.kind) != Some(TokenKind::Eof) {
            let end = tokens
                .last()
                .map_or(Span::DUMMY, |token| token.span.shrink_to_hi());
            tokens.push(Token::new(TokenKind::Eof, end));
        }

        let token = tokens[0];
        Parser {
            tokens,
            index: 0,
            token,
            prev_token: Token::new(TokenKind::Eof, token.span.shrink_to_lo()),
            expected: Vec::new(),
            dcx,
        }
    }

    fn bump(&mut self) {
        self.prev_token = self.token;
        if self.index + 1 < self.tokens.len() {
            self.index += 1;
        }
        self.token = self.tokens[self.index];
        self.expected.clear();
    }

    fn look_ahead(&self, distance: usize) -> TokenKind {
        if distance == 0 {
            return self.token.kind;
        }
        self.tokens
            .get(self.index + distance)
            .map_or(TokenKind::Eof, |token| token.kind)
    }

    fn check(&mut self, kind: TokenKind) -> bool {
        let found = self.token.kind == kind;
        if !found {
            self.expected.push(Expected::Token(kind));
        }
        found
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        let found = self.check(kind);
        if found {
            self.bump();
        }
        found
    }

    fn eat_noexpect(&mut self, kind: TokenKind) -> bool {
        let found = self.token.kind == kind;
        if found {
            self.bump();
        }
        found
    }

    fn check_keyword(&mut self, keyword: Symbol) -> bool {
        let found = self.token.is_keyword(keyword);
        if !found {
            self.expected.push(Expected::Keyword(keyword));
        }
        found
    }

    fn eat_keyword(&mut self, keyword: Symbol) -> bool {
        let found = self.check_keyword(keyword);
        if found {
            self.bump();
        }
        found
    }

    fn eat_keyword_noexpect(&mut self, keyword: Symbol) -> bool {
        let found = self.token.is_keyword(keyword);
        if found {
            self.bump();
        }
        found
    }

    fn expect(&mut self, kind: TokenKind) -> PResult<Span> {
        if self.eat(kind) {
            Ok(self.prev_token.span)
        } else {
            Err(self.unexpected())
        }
    }

    fn expect_semi(&mut self) -> PResult<()> {
        if self.eat(TokenKind::Semi) {
            return Ok(());
        }

        let message = self.unexpected_message();
        self.emit(
            Diagnostic::error(message)
                .with_span(self.prev_token.span.shrink_to_hi())
                .with_label("add `;` here"),
        );
        Ok(())
    }

    fn break_and_eat(&mut self, kind: TokenKind) -> bool {
        if self.token.kind == kind {
            self.bump();
            return true;
        }

        let Some((first, second)) = self.token.kind.break_two_token_op() else {
            return false;
        };
        if first != kind {
            return false;
        }

        let first_len = first.as_str().map_or(1, str::len) as u32;
        let span = self.token.span;
        let split = span.lo + first_len;
        self.prev_token = Token::new(first, Span::new(span.lo, split));
        self.token = Token::new(second, Span::new(split, span.hi));
        self.expected.clear();
        true
    }

    fn parse_ident(&mut self) -> PResult<Ident> {
        match self.token.ident() {
            Some(name) => {
                let span = self.token.span;
                self.bump();
                Ok(Ident::new(name, span))
            }
            None => {
                self.expected.push(Expected::Ident);
                Err(self.unexpected())
            }
        }
    }

    fn parse_docs(&mut self) -> Vec<Symbol> {
        let mut docs = Vec::new();
        while let TokenKind::DocComment(doc) = self.token.kind {
            docs.push(doc);
            self.bump();
        }
        docs
    }
}

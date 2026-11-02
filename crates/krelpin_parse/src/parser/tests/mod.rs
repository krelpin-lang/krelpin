use krelpin_ast::ast::{
    Class, CompilationUnit, FnRetTy, ItemKind, Member, MemberKind, Stmt, StmtKind,
};
use krelpin_errors::{DiagCtxt, DiagnosticBuffer};
use krelpin_span::SourceMap;

use crate::{lexer, parser};

mod decls;
mod exprs;
mod items;
mod pats;
mod stmts;

pub(super) struct Parsed {
    pub(super) unit: CompilationUnit,
    pub(super) errors: Vec<String>,
}

pub(super) fn parse_str(src: &str) -> Parsed {
    let source_map = SourceMap::new();
    let file = source_map.new_source_file("test.kr", src.to_owned());
    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());
    let tokens = lexer::lex(&file, &dcx);
    let unit = parser::parse(tokens, &dcx);
    Parsed {
        unit,
        errors: buffer.messages(),
    }
}

pub(super) fn parse_ok(src: &str) -> CompilationUnit {
    let parsed = parse_str(src);
    assert_eq!(
        parsed.errors,
        Vec::<String>::new(),
        "unexpected errors for {src:?}"
    );
    parsed.unit
}

pub(super) fn errors(src: &str) -> Vec<String> {
    parse_str(src).errors
}

pub(super) fn class(unit: &CompilationUnit, index: usize) -> &Class {
    match &unit.items[index].kind {
        ItemKind::Class(class) => class,
        other => panic!("expected a class, found {other:?}"),
    }
}

pub(super) fn members(src: &str) -> Vec<Member> {
    let unit = parse_ok(src);
    class(&unit, 0).members.clone()
}

pub(super) fn only_member(src: &str) -> MemberKind {
    let members = members(&format!("class A {{ {src} }}"));
    assert_eq!(members.len(), 1, "expected one member in {src:?}");
    members[0].kind.clone()
}

pub(super) fn ret_ty(src: &str) -> String {
    match only_member(src) {
        MemberKind::Method(method) => match method.ret {
            FnRetTy::Ty(ty) => ty.to_string(),
            FnRetTy::Void(_) => "void".to_owned(),
        },
        other => panic!("expected a method, found {other:?}"),
    }
}

pub(super) fn body(src: &str) -> Vec<Stmt> {
    let members = members(&format!("class A {{ void f(&this) {{ {src} }} }}"));
    match &members[0].kind {
        MemberKind::Method(method) => method.body.clone().expect("method has a body").stmts,
        other => panic!("expected a method, found {other:?}"),
    }
}

pub(super) fn body_errors(src: &str) -> Vec<String> {
    errors(&format!("class A {{ void f(&this) {{ {src} }} }}"))
}

pub(super) fn expr(src: &str) -> String {
    let stmts = body(&format!("Object t = {src};"));
    match &stmts[0].kind {
        StmtKind::Local(local) => local.init.as_ref().expect("initializer").to_string(),
        other => panic!("expected a local, found {other:?}"),
    }
}

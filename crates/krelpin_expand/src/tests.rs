use std::path::Path;

use krelpin_ast::ast::{CompilationUnit, ItemKind, MemberKind};
use krelpin_errors::{DiagCtxt, DiagnosticBuffer};
use krelpin_parse::{lexer, parser};
use krelpin_span::SourceMap;
use krelpin_sysroot::Sysroot;

struct Expanded {
    unit: CompilationUnit,
    errors: Vec<String>,
}

fn expand_str(src: &str) -> Expanded {
    let source_map = SourceMap::new();
    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut units = Sysroot::new(root)
        .expect("the workspace is a sysroot")
        .load_std(&source_map, &dcx);
    let file = source_map.new_source_file("test.kr", src.to_owned());
    units.push(parser::parse(lexer::lex(&file, &dcx), &dcx));
    crate::expand(&mut units, &source_map, &dcx);
    Expanded {
        unit: units.pop().expect("the test unit"),
        errors: buffer.messages(),
    }
}

fn errors(src: &str) -> Vec<String> {
    expand_str(src).errors
}

fn method_names(unit: &CompilationUnit) -> Vec<&str> {
    let members = match &unit.items[0].kind {
        ItemKind::Class(class) => &class.members,
        ItemKind::Record(def) => &def.members,
        ItemKind::Enum(def) => &def.members,
        ItemKind::Interface(interface) => &interface.members,
    };
    members
        .iter()
        .filter_map(|member| match &member.kind {
            MemberKind::Method(method) => Some(method.name.name.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn derive_adds_interfaces_and_methods() {
    let expanded = expand_str(
        "@Derive(Equatable, Hashable)\n\
         class Point { int x; int y; }",
    );
    assert_eq!(expanded.errors, Vec::<String>::new());
    let ItemKind::Class(class) = &expanded.unit.items[0].kind else {
        panic!("expected a class");
    };
    let interfaces: Vec<String> = class.implements.iter().map(|ty| ty.to_string()).collect();
    assert_eq!(interfaces, ["Equatable<Point>", "Hashable<Point>"]);
    assert_eq!(method_names(&expanded.unit), ["equals", "hashCode"]);
    assert!(expanded.unit.items[0].annotations.is_empty());
}

#[test]
fn derive_on_generic_enums() {
    let expanded = expand_str(
        "@Derive(Equatable)\n\
         enum Tree<T extends Equatable<T>> { Leaf(T value), Node(Tree<T> left, Tree<T> right) }",
    );
    assert_eq!(expanded.errors, Vec::<String>::new());
    let ItemKind::Enum(tree) = &expanded.unit.items[0].kind else {
        panic!("expected an enum");
    };
    assert_eq!(tree.implements[0].to_string(), "Equatable<Tree<T>>");
    assert_eq!(method_names(&expanded.unit), ["equals"]);
}

#[test]
fn derive_arguments_are_checked() {
    assert_eq!(
        errors("@Derive class A {}"),
        ["`@Derive` needs the interfaces to implement"]
    );
    assert_eq!(
        errors("@Derive(Cloneable) class A {}"),
        ["cannot derive `Cloneable`"]
    );
    assert_eq!(
        errors("@Derive(Equatable, Equatable) class A {}"),
        ["`Equatable` is derived more than once"]
    );
    assert_eq!(
        errors("@Derive(Equatable Hashable) class A {}"),
        ["expected the name of an interface"]
    );
}

#[test]
fn derive_targets_are_checked() {
    assert_eq!(
        errors("@Derive(Equatable) interface I {}"),
        ["`@Derive` cannot be used on interfaces"]
    );
    assert_eq!(
        errors("@Derive(Equatable) enum Color { Red, Green }"),
        ["enums without data are already `Equatable`"]
    );
    assert_eq!(
        errors("class A { @Derive(Equatable) int f(&this) { return 1; } }"),
        ["`@Derive` cannot be used on members"]
    );
    assert_eq!(
        errors("enum E { @Derive(Equatable) A(int x) }"),
        ["`@Derive` cannot be used on enum variants"]
    );
}

#[test]
fn macros_are_resolved_by_name() {
    assert_eq!(
        errors("@Builder class A {}"),
        ["cannot find macro `Builder` in this scope"]
    );
    assert_eq!(
        errors("@std.lang.Derive(Equatable) class A {}"),
        Vec::<String>::new()
    );
    let expanded = expand_str("import std.lang.Derive;\n@Derive(Equatable) class A {}");
    assert_eq!(expanded.errors, Vec::<String>::new());
    assert!(expanded.unit.imports.is_empty());
}

#[test]
fn only_the_standard_library_declares_macros() {
    assert_eq!(
        errors("macro Builder;"),
        ["only the standard library can declare macros for now"]
    );
}

#[test]
fn derive_comparable() {
    let expanded = expand_str("@Derive(Comparable) enum Priority { Low, High }");
    assert_eq!(expanded.errors, Vec::<String>::new());
    assert_eq!(method_names(&expanded.unit), ["compareTo"]);
    assert_eq!(
        errors("@Derive(Comparable) class A { String? label; }"),
        ["`Comparable` cannot be derived because `label` may be null"]
    );
}

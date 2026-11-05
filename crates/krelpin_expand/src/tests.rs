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
    expand_with(src, &crate::CfgSet::default())
}

fn expand_with(src: &str, cfg: &crate::CfgSet) -> Expanded {
    let source_map = SourceMap::new();
    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut units = Sysroot::new(root)
        .expect("the workspace is a sysroot")
        .load_std(&source_map, &dcx);
    let file = source_map.new_source_file("test.kr", src.to_owned());
    units.push(parser::parse(lexer::lex(&file, &dcx), &dcx));
    let options = crate::ExpandOptions {
        cfg,
        proc_macro: None,
        externs: &[],
    };
    crate::expand_crate(&mut units, &source_map, &dcx, &options);
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
        errors("class A { @Derive(Equatable) int f() { return 1; } }"),
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

fn expand_proc_macro(src: &str) -> Expanded {
    let source_map = SourceMap::new();
    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut units = Sysroot::new(root)
        .expect("the workspace is a sysroot")
        .load_std(&source_map, &dcx);
    let file = source_map.new_source_file("test.kr", src.to_owned());
    units.push(parser::parse(lexer::lex(&file, &dcx), &dcx));
    let options = crate::ExpandOptions {
        cfg: &crate::CfgSet::default(),
        proc_macro: Some("demo"),
        externs: &[],
    };
    crate::expand_crate(&mut units, &source_map, &dcx, &options);
    Expanded {
        unit: units.pop().expect("the test unit"),
        errors: buffer.messages(),
    }
}

const PROC_MACRO_IMPORTS: &str = "package demo;\nimport std.macros.TokenStream;\n";

#[test]
fn macros_are_defined_in_proc_macro_crates() {
    assert_eq!(
        errors("macro Builder;"),
        ["only the standard library can declare macros without a body"]
    );
    assert_eq!(
        errors("macro Builder(TokenStream args, TokenStream item) { return item; }"),
        ["macros can only be defined in a `proc-macro` crate"]
    );
}

#[test]
fn proc_macro_crates_get_a_macro_server() {
    let src = format!(
        "{PROC_MACRO_IMPORTS}macro Builder(TokenStream args, TokenStream item) {{ return item; }}\n\
         class Helper {{}}"
    );
    let expanded = expand_proc_macro(&src);
    assert_eq!(expanded.errors, Vec::<String>::new());
    assert!(expanded.unit.macros.is_empty());
    let names: Vec<&str> = expanded
        .unit
        .items
        .iter()
        .map(|item| item.name.name.as_str())
        .collect();
    assert_eq!(names, ["Helper", "macros#"]);
    let ItemKind::Class(server) = &expanded.unit.items[1].kind else {
        panic!("the macro server is a class");
    };
    let members: Vec<&str> = server
        .members
        .iter()
        .filter_map(|member| match &member.kind {
            MemberKind::Method(method) => Some(method.name.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(members, ["main", "Builder"]);
    let MemberKind::Method(builder) = &server.members[1].kind else {
        panic!("the macro is a method");
    };
    assert!(builder.receiver.is_none());
    assert_eq!(builder.params[1].name.name.as_str(), "item");
}

#[test]
fn macro_definitions_are_checked() {
    let src = format!(
        "{PROC_MACRO_IMPORTS}macro One(TokenStream args) {{ return args; }}\n\
         macro Two(TokenStream args, String item) {{ return args; }}\n\
         macro Three;\n\
         macro main(TokenStream args, TokenStream item) {{ return item; }}\n\
         class App {{ static void main() {{}} }}"
    );
    assert_eq!(
        expand_proc_macro(&src).errors,
        [
            "a `proc-macro` crate cannot have a `main` method",
            "the macro `One` takes 1 parameter, but macros take two",
            "macro parameters are `TokenStream`s",
            "the macro `Three` has no body",
            "a macro cannot be named `main`",
        ]
    );
}

#[test]
fn macros_cannot_be_used_where_they_are_defined() {
    let src = format!(
        "{PROC_MACRO_IMPORTS}macro Builder(TokenStream args, TokenStream item) {{ return item; }}\n\
         @Builder class A {{}}"
    );
    assert_eq!(
        expand_proc_macro(&src).errors,
        ["the macro `Builder` cannot be used in the crate that defines it"]
    );
}

#[test]
fn wire_tokens_round_trip() {
    use crate::proc_macro::{escape, unescape};
    use crate::{WireKind, WireToken};

    for text in [
        "plain",
        "tab\there",
        "line\nbreak",
        "back\\slash",
        "\\n",
        "\r\n\t",
    ] {
        assert_eq!(unescape(&escape(text)), text);
        assert!(!escape(text).contains(['\n', '\t', '\r']));
    }
    let token = WireToken {
        kind: WireKind::Doc,
        span: Some(7),
        text: "/** two\n lines */".to_owned(),
    };
    assert_eq!(WireToken::decode(&token.encode()), Some(token));
    let fresh = WireToken {
        kind: WireKind::Punct,
        span: None,
        text: "->".to_owned(),
    };
    assert_eq!(fresh.encode(), "punct\t-1\t->");
    assert_eq!(WireToken::decode(&fresh.encode()), Some(fresh));
    assert_eq!(WireToken::decode("bogus\t1\tx"), None);
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

#[test]
fn records_only_derive_comparable() {
    assert_eq!(
        errors("@Derive(Equatable) record Point(int x, int y);"),
        ["records are already `Equatable` when their components are"]
    );
    assert_eq!(
        errors("@Derive(Comparable) record Point(int x, int y);"),
        Vec::<String>::new()
    );
}

fn linux() -> crate::CfgSet {
    let mut cfg = crate::CfgSet::default();
    cfg.insert("unix", None);
    cfg.insert("target_os", Some("linux"));
    cfg.insert_spec("feature=\"fast\"").expect("a valid cfg");
    cfg
}

#[test]
fn cfg_keeps_only_matching_members() {
    let expanded = expand_with(
        "class A {\n\
         \x20   @Cfg(unix) void onUnix() {}\n\
         \x20   @Cfg(windows) void onWindows() {}\n\
         \x20   @Cfg(target_os = \"linux\") void onLinux() {}\n\
         \x20   @Cfg(not(feature = \"fast\")) void slow() {}\n\
         \x20   @Cfg(all(unix, any(windows, feature = \"fast\"))) void both() {}\n\
         }",
        &linux(),
    );
    assert_eq!(expanded.errors, Vec::<String>::new());
    assert_eq!(method_names(&expanded.unit), ["onUnix", "onLinux", "both"]);
}

#[test]
fn cfg_removes_whole_items() {
    let expanded = expand_with("@Cfg(windows) class A {} class B {}", &linux());
    assert_eq!(expanded.unit.items.len(), 1);
}

#[test]
fn cfg_reports_malformed_conditions() {
    assert_eq!(
        expand_with("class A { @Cfg(target_os = 5) void f() {} }", &linux()).errors,
        ["expected a string after `target_os =` in `@Cfg`"]
    );
    assert_eq!(
        expand_with("class A { @Cfg(unix, windows) void f() {} }", &linux()).errors,
        ["`@Cfg` takes exactly one condition"]
    );
}

#[test]
fn check_cfg_reports_unexpected_names_and_values() {
    let mut cfg = linux();
    cfg.add_check("cfg(feature, values(\"fast\", \"small\"))")
        .expect("a valid check");
    let expanded = expand_with(
        "class A {\n\
         \x20   @Cfg(feature = \"small\") void a() {}\n\
         \x20   @Cfg(feature = \"tiny\") void b() {}\n\
         \x20   @Cfg(fature) void c() {}\n\
         \x20   @Cfg(target_os = \"linux\") void d() {}\n\
         }",
        &cfg,
    );
    assert_eq!(
        expanded.errors,
        [
            "unexpected `@Cfg` value `\"tiny\"` for `feature`",
            "unexpected `@Cfg` condition name `fature`"
        ]
    );
    assert!(cfg.add_check("feature").is_err());
}

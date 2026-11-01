use krelpin_ast::ast::{
    Class, CompilationUnit, FieldStorage, FnRetTy, GenericParamKind, ItemKind, Member, MemberKind,
    ReceiverKind, Visibility,
};
use krelpin_errors::{DiagCtxt, DiagnosticBuffer};
use krelpin_span::SourceMap;

use crate::{lexer, parser};

struct Parsed {
    unit: CompilationUnit,
    errors: Vec<String>,
}

fn parse_str(src: &str) -> Parsed {
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

fn parse_ok(src: &str) -> CompilationUnit {
    let parsed = parse_str(src);
    assert_eq!(
        parsed.errors,
        Vec::<String>::new(),
        "unexpected errors for {src:?}"
    );
    parsed.unit
}

fn errors(src: &str) -> Vec<String> {
    parse_str(src).errors
}

fn class(unit: &CompilationUnit, index: usize) -> &Class {
    match &unit.items[index].kind {
        ItemKind::Class(class) => class,
    }
}

fn members(src: &str) -> Vec<Member> {
    let unit = parse_ok(src);
    class(&unit, 0).members.clone()
}

fn only_member(src: &str) -> MemberKind {
    let members = members(&format!("class A {{ {src} }}"));
    assert_eq!(members.len(), 1, "expected one member in {src:?}");
    members[0].kind.clone()
}

fn ret_ty(src: &str) -> String {
    match only_member(src) {
        MemberKind::Method(method) => match method.ret {
            FnRetTy::Ty(ty) => ty.to_string(),
            FnRetTy::Void(_) => "void".to_owned(),
        },
        other => panic!("expected a method, found {other:?}"),
    }
}

#[test]
fn package_and_imports() {
    let unit = parse_ok("package a.b;\nimport a.c.D;\nimport a.e.*;\nclass X {}");
    assert_eq!(unit.package.unwrap().to_string(), "a.b");

    let imports: Vec<(String, bool)> = unit
        .imports
        .iter()
        .map(|import| (import.path.to_string(), import.glob))
        .collect();
    assert_eq!(
        imports,
        [("a.c.D".to_owned(), false), ("a.e".to_owned(), true)]
    );
    assert_eq!(unit.items.len(), 1);
}

#[test]
fn hello_world() {
    let unit = parse_ok(
        "package example;\n\
         class Main {\n\
         \x20   void main() {\n\
         \x20       Console.println(\"Hello World!\");\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(unit.items[0].name.name.as_str(), "Main");

    let MemberKind::Method(main) = &class(&unit, 0).members[0].kind else {
        panic!("expected main method");
    };
    assert_eq!(main.name.name.as_str(), "main");
    assert!(matches!(main.ret, FnRetTy::Void(_)));
    assert!(main.receiver.is_none());
    assert!(main.body.is_some());
}

#[test]
fn fields() {
    let members = members(
        "class Person {\n\
         \x20   const int MAX = 64;\n\
         \x20   shared AtomicInt created = new AtomicInt(0);\n\
         \x20   String name;\n\
         \x20   public int age;\n\
         \x20   private String? nickname;\n\
         }",
    );

    let summary: Vec<(String, String, bool)> = members
        .iter()
        .map(|member| match &member.kind {
            MemberKind::Field(field) => {
                let storage = match field.storage {
                    FieldStorage::Instance => "instance",
                    FieldStorage::Const(_) => "const",
                    FieldStorage::Shared(_) => "shared",
                };
                (
                    storage.to_owned(),
                    format!("{} {}", field.ty, field.name.name),
                    field.init.is_some(),
                )
            }
            other => panic!("expected a field, found {other:?}"),
        })
        .collect();

    let expected = [
        ("const", "int MAX", true),
        ("shared", "AtomicInt created", true),
        ("instance", "String name", false),
        ("instance", "int age", false),
        ("instance", "String? nickname", false),
    ];
    let expected: Vec<(String, String, bool)> = expected
        .iter()
        .map(|&(storage, decl, init)| (storage.to_owned(), decl.to_owned(), init))
        .collect();
    assert_eq!(summary, expected);

    assert!(matches!(members[3].vis, Visibility::Public(_)));
    assert!(matches!(members[4].vis, Visibility::Private(_)));
    assert_eq!(members[2].vis, Visibility::Default);
}

#[test]
fn constructors() {
    let members = members("class Person { Person(String name, int age) { this.name = name; } }");
    let MemberKind::Constructor(constructor) = &members[0].kind else {
        panic!("expected a constructor");
    };
    assert_eq!(constructor.params.len(), 2);
}

#[test]
fn method_receivers() {
    let members = members(
        "class Person {\n\
         \x20   &String getName(&this) { return &this.name; }\n\
         \x20   void rename(&mut this, String newName) {}\n\
         \x20   String intoName(this) { return this.name; }\n\
         \x20   int max(int a, int b) { return a; }\n\
         }",
    );

    let receivers: Vec<Option<ReceiverKind>> = members
        .iter()
        .map(|member| match &member.kind {
            MemberKind::Method(method) => method.receiver.map(|receiver| receiver.kind),
            other => panic!("expected a method, found {other:?}"),
        })
        .collect();
    assert_eq!(
        receivers,
        [
            Some(ReceiverKind::Ref),
            Some(ReceiverKind::RefMut),
            Some(ReceiverKind::Owned),
            None,
        ]
    );
}

#[test]
fn reference_types() {
    assert_eq!(ret_ty("&String f() {}"), "&String");
    assert_eq!(ret_ty("&mut [a, b] String f() {}"), "&mut [a, b] String");
    assert_eq!(ret_ty("&String? f() {}"), "&String?");
    assert_eq!(ret_ty("&(String?) f() {}"), "&(String?)");
    assert_eq!(ret_ty("&&String f() {}"), "&&String");
}

#[test]
fn generic_types_split_closing_brackets() {
    assert_eq!(
        ret_ty("Map<String, List<&String>> f() {}"),
        "Map<String, List<&String>>"
    );
    assert_eq!(ret_ty("A<B<C<D>>> f() {}"), "A<B<C<D>>>");
    assert_eq!(ret_ty("java.util.List<int> f() {}"), "java.util.List<int>");
}

#[test]
fn class_generics() {
    let unit = parse_ok("class Parser<&src, T extends Comparable<T> & Cloneable> {}");
    let params = &class(&unit, 0).generics.params;

    assert_eq!(params[0].name.name.as_str(), "src");
    assert_eq!(params[0].kind, GenericParamKind::BorrowSource);

    let GenericParamKind::Type { bounds } = &params[1].kind else {
        panic!("expected a type parameter");
    };
    let bounds: Vec<String> = bounds.iter().map(ToString::to_string).collect();
    assert_eq!(bounds, ["Comparable<T>", "Cloneable"]);
}

#[test]
fn extends_and_implements() {
    let unit = parse_ok("class Dog extends Animal implements Pet, Comparable<Dog> {}");
    let class = class(&unit, 0);
    assert_eq!(class.extends.as_ref().unwrap().to_string(), "Animal");
    let implements: Vec<String> = class.implements.iter().map(ToString::to_string).collect();
    assert_eq!(implements, ["Pet", "Comparable<Dog>"]);
}

#[test]
fn generic_methods_varargs_throws_and_abstract_methods() {
    let MemberKind::Method(first) = only_member("<T> T first(List<T> items) {}") else {
        panic!("expected a method");
    };
    assert_eq!(first.generics.params.len(), 1);

    let MemberKind::Method(log) = only_member("void log(String... parts) {}") else {
        panic!("expected a method");
    };
    assert!(log.params[0].variadic);

    let MemberKind::Method(read) = only_member("void read(&this) throws IOException, Bad {}")
    else {
        panic!("expected a method");
    };
    assert_eq!(read.throws.len(), 2);

    let MemberKind::Method(run) = only_member("void run(&this);") else {
        panic!("expected a method");
    };
    assert!(run.body.is_none());
}

#[test]
fn docs_and_annotations() {
    let members =
        members("class A { /** Docs */ @Override @Since(\"1.0\") String text(&this) {} }");
    assert_eq!(members[0].docs.len(), 1);
    assert_eq!(members[0].docs[0].as_str(), " Docs ");

    let annotations: Vec<(String, bool)> = members[0]
        .annotations
        .iter()
        .map(|annotation| (annotation.path.to_string(), annotation.args.is_some()))
        .collect();
    assert_eq!(
        annotations,
        [("Override".to_owned(), false), ("Since".to_owned(), true)]
    );
}

#[test]
fn docs_before_package_belong_to_unit() {
    let unit = parse_ok("/** Unit */ package a; /** Class */ class A {}");
    assert_eq!(unit.docs.len(), 1);
    assert_eq!(unit.items[0].docs.len(), 1);
}

#[test]
fn signature_errors() {
    assert_eq!(
        errors("class A { foo() {} }"),
        ["missing return type for method"]
    );
    assert_eq!(
        errors("class A { void x; }"),
        ["fields cannot have type `void`"]
    );
    assert_eq!(
        errors("class A { void f(int a, &this) {} }"),
        ["`this` must be the first parameter"]
    );
    assert_eq!(
        errors("class A { A(&this) {} }"),
        ["constructors cannot take `this` as a parameter"]
    );
}

#[test]
fn modifier_errors() {
    assert_eq!(
        errors("class A { const void f() {} }"),
        ["`const` cannot be used on methods"]
    );
    assert_eq!(
        errors("shared class A {}"),
        ["`shared` cannot be used on classes"]
    );
    assert_eq!(
        errors("class A { public private int x; }"),
        ["multiple visibility modifiers"]
    );
    assert_eq!(
        errors("class A { const shared int x; }"),
        ["a field can only have one of `const` and `shared`"]
    );
}

#[test]
fn expected_token_messages() {
    assert_eq!(
        errors("class A { int x }"),
        ["expected one of `(`, `=`, or `;`, found `}`"]
    );
    assert_eq!(
        errors("class A { int class; }"),
        ["expected identifier, found keyword `class`"]
    );
    assert_eq!(
        errors("class A { int x = ; }"),
        ["expected expression, found `;`"]
    );
    assert_eq!(
        errors("class A { List<String x; }"),
        ["expected `,` or `>`, found `x`"]
    );
}

#[test]
fn unclosed_class() {
    assert_eq!(
        errors("class A { int x;"),
        ["this file contains an unclosed delimiter"]
    );
    assert_eq!(
        errors("class A { void f() { if (x) { }"),
        ["this file contains an unclosed delimiter"]
    );
}

#[test]
fn recovers_at_top_level() {
    let parsed = parse_str("int junk; class A {} 42 class B {}");
    assert_eq!(
        parsed.errors,
        [
            "expected class declaration, found keyword `int`",
            "expected class declaration, found `42`"
        ]
    );
    let names: Vec<&str> = parsed
        .unit
        .items
        .iter()
        .map(|item| item.name.name.as_str())
        .collect();
    assert_eq!(names, ["A", "B"]);
}

#[test]
fn recovers_inside_class() {
    let parsed = parse_str("class A { int 5; void broken( { } String ok; }");
    assert_eq!(parsed.errors.len(), 2);

    let names: Vec<String> = class(&parsed.unit, 0)
        .members
        .iter()
        .map(|member| match &member.kind {
            MemberKind::Field(field) => field.name.name.to_string(),
            other => panic!("unexpected member {other:?}"),
        })
        .collect();
    assert_eq!(names, ["ok"]);
}

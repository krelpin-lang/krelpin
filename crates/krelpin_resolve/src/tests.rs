use std::path::Path;

use krelpin_errors::{DiagCtxt, DiagnosticBuffer};
use krelpin_parse::{lexer, parser};
use krelpin_span::{BytePos, SourceMap, Span, Symbol};
use krelpin_sysroot::Sysroot;

use crate::{PackagePath, Res, ResolverOutput, resolve};

struct Resolved {
    src: String,
    output: ResolverOutput,
    errors: Vec<String>,
    helps: Vec<String>,
}

impl Resolved {
    fn res_at(&self, needle: &str, nth: usize) -> Option<Res> {
        let start = self.src.match_indices(needle).nth(nth)?.0 as u32;
        let len = needle.chars().take_while(|c| c.is_alphanumeric()).count() as u32;
        let span = Span::new(BytePos(start), BytePos(start + len));
        self.output.resolutions.get(span)
    }

    fn local_type(&self, name: &str) -> Option<Res> {
        let def = self
            .output
            .items
            .lookup(&PackagePath::default(), Symbol::intern(name))?;
        Some(Res::Item(def))
    }

    fn std_type(&self, path: &str) -> Option<Res> {
        self.output.items.lookup_path(path).map(Res::Item)
    }
}

fn test_sysroot() -> Sysroot {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Sysroot::new(root).expect("the workspace is a sysroot")
}

fn resolve_str(src: &str) -> Resolved {
    let source_map = SourceMap::new();
    let file = source_map.new_source_file("test.kr", src.to_owned());
    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());
    let unit = parser::parse(lexer::lex(&file, &dcx), &dcx);
    let mut units = test_sysroot().load_std(&source_map, &dcx);
    units.push(unit);
    assert_eq!(
        buffer.messages(),
        Vec::<String>::new(),
        "parse errors in {src:?}"
    );

    let output = resolve(&units, &dcx);
    let diagnostics = buffer.diagnostics();
    Resolved {
        src: src.to_owned(),
        output,
        errors: diagnostics.iter().map(|d| d.message.clone()).collect(),
        helps: diagnostics
            .iter()
            .flat_map(|d| d.children.iter().map(|child| child.message.clone()))
            .collect(),
    }
}

fn errors(src: &str) -> Vec<String> {
    resolve_str(src).errors
}

fn in_method(body: &str) -> Vec<String> {
    errors(&format!(
        "class A {{ int count; void run(&mut this, int arg) {{ {body} }} }}"
    ))
}

fn ok(src: &str) {
    assert_eq!(
        errors(src),
        Vec::<String>::new(),
        "unexpected errors for {src:?}"
    );
}

#[test]
fn hello_world_resolves() {
    ok("package example;\n\
        class Main {\n\
        \x20   static void main() {\n\
        \x20       Console.println(\"Hello World!\");\n\
        \x20   }\n\
        }\n");
}

#[test]
fn locals_params_and_types_resolve() {
    let resolved = resolve_str(
        "class Person {\n\
         \x20   String name;\n\
         \x20   Person(String name) { this.name = name; }\n\
         \x20   void greet(&this, Person other) {\n\
         \x20       String text = other.name;\n\
         \x20       Console.println(text);\n\
         \x20   }\n\
         }",
    );
    assert_eq!(resolved.errors, Vec::<String>::new());

    let param = resolved.src.find("name) {").unwrap() as u32;
    assert_eq!(
        resolved.res_at("name;", 1),
        Some(Res::Local(Span::new(BytePos(param), BytePos(param + 4))))
    );
    assert_eq!(resolved.res_at("Person", 2), resolved.local_type("Person"));
    assert_eq!(
        resolved.res_at("Console", 0),
        resolved.std_type("std.lang.Console")
    );
}

#[test]
fn unknown_names() {
    assert_eq!(
        in_method("Strng s = null;"),
        ["cannot find type `Strng` in this scope"]
    );
    assert_eq!(
        in_method("int x = missing;"),
        ["cannot find value `missing` in this scope"]
    );
    assert_eq!(
        in_method("missingCall();"),
        ["cannot find method `missingCall` in this scope"]
    );
}

#[test]
fn fields_need_explicit_this() {
    let resolved = resolve_str("class A { int count; void run(&mut this) { count = 1; } }");
    assert_eq!(resolved.errors, ["cannot find value `count` in this scope"]);
    assert_eq!(
        resolved.helps,
        ["to use the field of this object, write `this.count`"]
    );

    let resolved = resolve_str("class A { const int MAX = 1; int f(&this) { return MAX; } }");
    assert_eq!(
        resolved.helps,
        ["to use the field of the class, write `A.MAX`"]
    );
}

#[test]
fn this_is_only_available_with_a_receiver() {
    ok("class A { int x; A() { this.x = 1; } int get(&this) { return this.x; } }");
    assert_eq!(
        errors("class A { int x; static int get() { return this.x; } }"),
        ["`this` is not available here"]
    );
    assert_eq!(
        errors("class A { static A instance = this; }"),
        ["`this` is not available here"]
    );
}

#[test]
fn instance_methods_need_an_object() {
    ok("class A { void helper(&this) {} void run(&this) { this.helper(); } }");
    ok("class A { static int twice(int x) { return x * 2; } int run(&this) { return twice(2); } }");

    let resolved = resolve_str("class A { void helper(&this) {} void run(&this) { helper(); } }");
    assert_eq!(
        resolved.errors,
        ["method `helper` needs an object to be called on"]
    );
    assert_eq!(
        resolved.helps,
        ["call it on this object: `this.helper(...)`"]
    );
}

#[test]
fn duplicate_definitions() {
    assert_eq!(
        errors("class A {} class A {}"),
        ["the name `A` is defined multiple times"]
    );
    assert_eq!(
        errors("import a.List; class List {}"),
        ["the name `List` is defined multiple times"]
    );
    assert_eq!(
        errors("class A { int x; String x; }"),
        ["the field `x` is defined multiple times"]
    );
    assert_eq!(
        errors("enum E { A, B, A }"),
        ["the variant `A` is defined multiple times"]
    );
    assert_eq!(
        errors("enum E { A(int x), B; static void A() {} }"),
        ["the variant `A` has the same name as a member of the enum"]
    );
    assert_eq!(
        errors("class A<T, T> {}"),
        ["the generic parameter `T` is declared multiple times"]
    );
}

#[test]
fn methods_can_be_overloaded() {
    ok("class A { void f(&this) {} void f(&this, int x) {} }");
}

#[test]
fn locals_cannot_shadow() {
    assert_eq!(
        in_method("int arg = 1;"),
        ["a variable named `arg` is already defined in this scope"]
    );
    assert_eq!(
        in_method("int x = 1; { int x = 2; }"),
        ["a variable named `x` is already defined in this scope"]
    );
    assert_eq!(
        in_method("int x = 1; Runnable r = () -> { int x = 2; };"),
        ["a variable named `x` is already defined in this scope"]
    );
    assert_eq!(
        in_method("{ int x = 1; } { int x = 2; }"),
        Vec::<String>::new()
    );
    assert_eq!(
        in_method("for (int i = 0; i < 3; i++) {} for (int i = 0; i < 3; i++) {}"),
        Vec::<String>::new()
    );
}

#[test]
fn locals_are_not_visible_outside_their_block() {
    assert_eq!(
        in_method("{ int x = 1; } int y = x;"),
        ["cannot find value `x` in this scope"]
    );
}

#[test]
fn instanceof_bindings_are_scoped_to_the_if() {
    assert_eq!(
        in_method(
            "Object o = null; \
             if (o instanceof String s) { Console.println(s); } \
             if (o instanceof String s) { Console.println(s); }"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn generics_and_borrow_sources() {
    ok("class Box<T> { T value; <U> U map(&this, U other) { return other; } }");
    ok("class Parser<&src> { &[src] String input; }");
    ok("class A { static &[a, b] String longer(&String a, &String b) { return a; } }");
    ok("class A { String text; &[this] String get(&this) { return &this.text; } }");

    assert_eq!(
        errors("class A { static &[c] String f(&String a) { return a; } }"),
        ["cannot find borrow source `c`"]
    );
    assert_eq!(
        errors("class Box<T> { static T instance = null; }"),
        ["generic parameter `T` cannot be used here"]
    );
    assert_eq!(
        errors("class Box<T> { static T make() { return null; } }"),
        ["generic parameter `T` cannot be used here"]
    );
}

#[test]
fn jumps_must_be_in_the_right_place() {
    assert_eq!(in_method("break;"), ["`break` outside of a loop or switch"]);
    assert_eq!(in_method("continue;"), ["`continue` outside of a loop"]);
    assert_eq!(
        in_method("yield 1;"),
        ["`yield` outside of a switch expression"]
    );
    assert_eq!(
        in_method("while (true) { Runnable r = () -> { break; }; }"),
        ["`break` outside of a loop or switch"]
    );
    assert_eq!(
        in_method("int x = switch (arg) { default -> { return; } };"),
        ["cannot `return` from inside a switch expression"]
    );
    assert_eq!(
        in_method(
            "while (true) { if (arg > 0) { continue; } break; } \
             switch (arg) { case 1 -> { break; } default -> {} } \
             int y = switch (arg) { case 1 -> 1; default -> { yield 2; } };"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn enum_constants_in_case_labels() {
    ok("enum Color { RED, GREEN }\n\
        class A {\n\
        \x20   static String name(Color color) {\n\
        \x20       return switch (color) { case RED -> \"red\"; case GREEN -> \"green\"; };\n\
        \x20   }\n\
        }");
}

#[test]
fn glob_imports_suppress_unknown_names() {
    ok(
        "import java.util.*; class A { static Optional<String> find() { return \
         Optional.empty(); } }",
    );
}

#[test]
fn catch_and_lambda_parameters() {
    assert_eq!(
        in_method(
            "try { arg = 1; } catch (Exception e) { Console.println(e); } \
             List<int> xs = null; xs.forEach(x -> Console.println(x + arg));"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn prelude_types_come_from_the_standard_library() {
    let resolved = resolve_str(
        "class A { List<String> names; AtomicInt count; Object any; Comparable<A> order; }",
    );
    assert_eq!(resolved.errors, Vec::<String>::new());
    assert_eq!(
        resolved.res_at("List", 0),
        resolved.std_type("std.util.List")
    );
    assert_eq!(
        resolved.res_at("String", 0),
        resolved.std_type("std.lang.String")
    );
    assert_eq!(
        resolved.res_at("AtomicInt", 0),
        resolved.std_type("std.sync.AtomicInt")
    );
}

#[test]
fn types_outside_the_prelude_need_an_import() {
    assert_eq!(
        errors("class A { static void f() throws IOException {} }"),
        ["cannot find type `IOException` in this scope"]
    );

    let resolved =
        resolve_str("import std.io.IOException; class A { static void f() throws IOException {} }");
    assert_eq!(resolved.errors, Vec::<String>::new());
    assert_eq!(
        resolved.res_at("IOException {", 0),
        resolved.std_type("std.io.IOException")
    );

    ok("import std.io.*; class A { static void f() throws IOException {} }");
}

#[test]
fn imports_from_known_packages_are_checked() {
    assert_eq!(
        errors("import std.util.Lst; class A {}"),
        ["cannot find type `Lst` in package `std.util`"]
    );
    assert_eq!(
        errors("import std.io.*; class A { Strem s; }"),
        ["cannot find type `Strem` in this scope"]
    );
    ok("import shop.money.Price; class A { Price price; }");
}

#[test]
fn local_types_shadow_the_prelude() {
    let resolved = resolve_str("class String {} class A { String text; }");
    assert_eq!(resolved.errors, Vec::<String>::new());
    assert_eq!(
        resolved.res_at("String text", 0),
        resolved.local_type("String")
    );
}

#[test]
fn only_the_standard_library_declares_intrinsics() {
    let resolved = resolve_str("class A { @Intrinsic int f(&this); }");
    assert_eq!(
        resolved.errors,
        ["only the standard library can declare intrinsics"]
    );
}

#[test]
fn where_clauses_constrain_generic_parameters() {
    ok("class Bag<T> {\n\
        \x20   boolean has(&this, T value) where T extends Comparable<T> { return true; }\n\
        }");
    assert_eq!(
        errors("class Bag<T> { boolean has(&this, T value) where U extends Comparable<U> {} }"),
        ["cannot find generic parameter `U`"]
    );
}

#[test]
fn several_patterns_cannot_bind() {
    assert_eq!(
        errors(
            "enum E { A(int x), B }\n\
             class C { static int f(E e) { return switch (e) { case A(int x), B -> 1; }; } }"
        ),
        ["a case with several patterns cannot bind variables"]
    );
}

#[test]
fn pattern_bindings_are_scoped_to_their_case() {
    assert_eq!(
        errors(
            "enum E { A(int x), B }\n\
             class C { static int f(E e) {\n\
             \x20   int y = switch (e) { case A(int x) when x > 0 -> x; default -> 0; };\n\
             \x20   return x;\n\
             } }"
        ),
        ["cannot find value `x` in this scope"]
    );
}

#[test]
fn nested_types_resolve_inside_and_through_their_outer_type() {
    ok("class List {\n\
        \x20   static class Node { Node? next; }\n\
        \x20   Node? head;\n\
        }\n\
        class Main { static void run() { List.Node node = null; } }");
    assert_eq!(
        errors("class List { static class Node {} }\nclass Main { Node node; }"),
        ["cannot find type `Node` in this scope"]
    );
    assert_eq!(
        errors("class List {}\nclass Main { List.Node node; }"),
        ["cannot find type `Node` in `List`"]
    );
    assert_eq!(
        errors("class List { static class Node {} enum Node { A } }"),
        ["the type `Node` is defined multiple times"]
    );
}

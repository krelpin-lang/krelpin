use std::path::Path;

use krelpin_errors::{DiagCtxt, DiagnosticBuffer};
use krelpin_mir::BodyId;
use krelpin_parse::{lexer, parser};
use krelpin_span::SourceMap;
use krelpin_sysroot::Sysroot;

fn errors(src: &str) -> Vec<String> {
    let source_map = SourceMap::new();
    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut units = Sysroot::new(root)
        .expect("the workspace is a sysroot")
        .load_std(&source_map, &dcx);
    let file = source_map.new_source_file("test.kr", src.to_owned());
    units.push(parser::parse(lexer::lex(&file, &dcx), &dcx));
    krelpin_expand::expand(&mut units, &source_map, &dcx);
    let resolved = krelpin_resolve::resolve(&units, &dcx);
    let typeck = krelpin_typeck::check(&units, &resolved, &dcx);
    assert_eq!(
        buffer.messages(),
        Vec::<String>::new(),
        "errors before borrowck in {src:?}"
    );

    let hir = krelpin_hir::lower(&units, &resolved, &typeck.tcx, &typeck.results);
    let mir = krelpin_mir::build(&hir, &typeck.tcx);
    let bodies = (0..mir.bodies.len()).map(|index| BodyId(index as u32));
    crate::check(&mir, bodies, &typeck.tcx, &dcx);
    buffer.messages()
}

fn in_main(code: &str) -> Vec<String> {
    errors(&format!(
        "import std.util.List;\n\
         import std.util.ArrayList;\n\
         import std.util.Iterator;\n\
         import std.function.Supplier;\n\
         class Box {{\n\
         \x20   public int count;\n\
         \x20   public List<String> items = new ArrayList<>();\n\
         \x20   mut void bump() {{ this.count++; }}\n\
         \x20   &List<String> view() {{ return &this.items; }}\n\
         }}\n\
         class Main {{\n\
         \x20   static void take(Box box) {{}}\n\
         \x20   static void run(Box input, List<String> names) {{ {code} }}\n\
         }}"
    ))
}

fn ok(code: &str) {
    assert_eq!(
        in_main(code),
        Vec::<String>::new(),
        "unexpected errors in {code:?}"
    );
}

#[test]
fn the_standard_library_is_accepted() {
    assert_eq!(errors("class Empty {}"), Vec::<String>::new());
}

#[test]
fn moved_values_cannot_be_used() {
    assert_eq!(
        in_main("Box other = input; input.bump();"),
        ["borrow of moved value: `input`"]
    );
    assert_eq!(
        in_main("Main.take(input); Main.take(input);"),
        ["use of moved value: `input`"]
    );
    assert_eq!(
        in_main("for (int i = 0; i < 2; i++) { Main.take(input); }"),
        ["use of moved value: `input`"]
    );
    assert_eq!(
        in_main("List<String> items = input.items; input.items.size(); Main.take(input);"),
        [
            "borrow of moved value: `input.items`",
            "use of partially moved value: `input`"
        ]
    );
}

#[test]
fn reassigning_restores_a_moved_value() {
    ok("Box other = input; input = new Box(); input.bump(); other.bump();");
    ok("if (names.isEmpty()) { Main.take(input); } else { input.bump(); }");
}

#[test]
fn copyable_values_are_copied() {
    ok("String a = \"x\"; String b = a; String c = a + b; int n = input.count; int m = n;");
}

#[test]
fn borrowed_values_cannot_be_moved_out() {
    assert_eq!(
        errors(
            "import std.util.List;\n\
             class Bag { List<String> items; List<String> items() { return this.items; } }"
        ),
        ["cannot move out of `this.items`, which is borrowed"]
    );
    assert_eq!(
        errors("class A { static Object first(Object[] values) { return values[0]; } }"),
        ["cannot move out of an element of `values`"]
    );
}

#[test]
fn changes_need_mut() {
    assert_eq!(
        errors("class A { int n; void f() { this.n = 1; } }"),
        ["cannot assign to `this.n` in a method that is not `mut`"]
    );
    assert_eq!(
        errors("class A { mut void f() {} void g() { this.f(); } }"),
        ["cannot borrow `this` as mutable in a method that is not `mut`"]
    );
    assert_eq!(
        errors("class A { int n; static void f(&A a) { a.n = 1; } }"),
        ["cannot assign to `a.n`, because it is behind a `&` reference"]
    );
    assert_eq!(
        errors(
            "class A { int n;\n\
             \x20   mut void f() { this.n = 1; }\n\
             \x20   static void g(&mut A a) { a.n = 2; a.f(); }\n\
             }"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn borrows_conflict_while_they_are_used() {
    assert_eq!(
        in_main("&List<String> view = input.view(); input.bump(); view.size();"),
        ["cannot borrow `input` as mutable because it is also borrowed as shared"]
    );
    ok("&List<String> view = input.view(); view.size(); input.bump();");
    assert_eq!(
        in_main("for (String name : names) { names.add(name); }"),
        ["cannot borrow `names` as mutable because it is also borrowed as shared"]
    );
}

#[test]
fn calls_borrow_in_two_phases() {
    ok("names.add(names.get(0)); input.items.add(input.items.get(0));");
}

#[test]
fn references_cannot_outlive_their_values() {
    assert_eq!(
        in_main("&String? kept = null; { String text = \"x\"; kept = &text; } kept?.length();"),
        ["`text` does not live long enough"]
    );
    assert_eq!(
        in_main(
            "List<&String> refs = new ArrayList<>();\n\
             { String text = \"x\"; refs.add(&text); }\n\
             refs.size();"
        ),
        ["`text` does not live long enough"]
    );
}

#[test]
fn lambdas_borrow_or_copy_their_captures() {
    ok("int calls = 0; names.forEach(name -> calls++); int seen = calls;");
    assert_eq!(
        in_main("int calls = 0; Runnable task = () -> calls++; calls = 5; task.run();"),
        ["cannot assign to `calls` because it is borrowed"]
    );
    assert_eq!(
        errors(
            "import std.util.List;\n\
             import std.function.Supplier;\n\
             class A { static Supplier<int> f(List<String> names) { return () -> names.size(); } }"
        ),
        ["a returned lambda borrows `names`, which belongs to this method"]
    );
    assert_eq!(
        errors(
            "import std.util.List;\n\
             import std.function.Supplier;\n\
             class A { static Supplier<int> f(List<String> names) {\n\
             \x20   return move () -> names.size();\n\
             } }"
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        errors(
            "import std.util.List;\n\
             import std.function.Supplier;\n\
             class A { static Supplier<List<String>> f(List<String> names) {\n\
             \x20   return move () -> names;\n\
             } }"
        ),
        ["cannot move `names` out of a lambda"]
    );
}

#[test]
fn patterns_borrow_what_they_cannot_move() {
    assert_eq!(
        errors(
            "import std.util.List;\n\
             enum Tree { Node(List<Tree> children), Leaf;\n\
             \x20   int size() {\n\
             \x20       return switch (this) {\n\
             \x20           case Node(List<Tree> children) -> children.size();\n\
             \x20           case Leaf -> 0;\n\
             \x20       };\n\
             \x20   }\n\
             }"
        ),
        Vec::<String>::new()
    );
}

#[test]
fn values_must_be_assigned_before_use() {
    assert_eq!(
        in_main("int count; if (names.isEmpty()) { count = 1; } int copy = count;"),
        ["`count` is used before it has a value"]
    );
}

#[test]
fn iterators_borrow_their_collection() {
    assert_eq!(
        in_main(
            "Iterator<String> values = names.iterator(); \
             names.add(\"x\"); \
             values.next();"
        ),
        ["cannot borrow `names` as mutable because it is also borrowed as shared"]
    );
}

#[test]
fn results_borrow_from_their_declared_sources() {
    let src = |code: &str| {
        errors(&format!(
            "class A {{\n\
             \x20   static &[a] String first(&String a, &String b) {{ return a; }}\n\
             \x20   static &[a, b] String either(&String a, &String b) {{ return b; }}\n\
             \x20   static int run() {{\n\
             \x20       String a = \"a\";\n\
             \x20       &String kept;\n\
             \x20       {{ {code} }}\n\
             \x20       return kept.length();\n\
             \x20   }}\n\
             }}"
        ))
    };
    assert_eq!(
        src("String b = \"b\"; kept = A.first(&a, &b);"),
        Vec::<String>::new()
    );
    assert_eq!(
        src("String b = \"b\"; kept = A.either(&a, &b);"),
        ["`b` does not live long enough"]
    );
}

#[test]
fn methods_keep_to_their_declared_sources() {
    assert_eq!(
        errors("class A { static &[a] String pick(&String a, &String b) { return b; } }"),
        ["`pick` returns a value borrowed from `b`, but its signature only borrows from `a`"]
    );
    assert_eq!(
        errors(
            "class Parser[src] {\n\
             \x20   &[src] String input;\n\
             \x20   String name = \"parser\";\n\
             \x20   Parser(&[src] String input) { this.input = input; }\n\
             \x20   &[src] String input() { return this.input; }\n\
             \x20   &[this] String name() { return &this.name; }\n\
             \x20   &[src] String wrong() { return &this.name; }\n\
             }"
        ),
        ["`wrong` returns a value borrowed from `this`, but its signature only borrows from `src`"]
    );
}

#[test]
fn objects_keep_the_borrows_they_store() {
    let src = |code: &str| {
        errors(&format!(
            "class Parser[src] {{\n\
             \x20   &[src] String input;\n\
             \x20   Parser(&[src] String input) {{ this.input = input; }}\n\
             \x20   &[src] String input() {{ return this.input; }}\n\
             }}\n\
             class Main {{ static int run() {{ {code} }} }}"
        ))
    };
    assert_eq!(
        src(
            "String text = \"abc\"; &String kept; { Parser parser = new Parser(&text); \
             kept = parser.input(); } return kept.length();"
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        src(
            "Parser parser; { String text = \"abc\"; parser = new Parser(&text); } \
             return parser.input().length();"
        ),
        ["`text` does not live long enough"]
    );
}

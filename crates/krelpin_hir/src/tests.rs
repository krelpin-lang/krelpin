use std::path::Path;

use krelpin_errors::{DiagCtxt, DiagnosticBuffer};
use krelpin_parse::{lexer, parser};
use krelpin_span::SourceMap;
use krelpin_sysroot::Sysroot;

fn lower_str(src: &str) -> String {
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
    assert_eq!(buffer.messages(), Vec::<String>::new(), "errors in {src:?}");

    let hir = crate::lower(&units, &resolved, &typeck.tcx, &typeck.results);
    let user_unit = units.len() - 1;
    let bodies = hir
        .bodies
        .iter()
        .filter(|body| resolved.items.def(body.owner).unit == user_unit);
    crate::print(bodies, &typeck.tcx)
}

fn assert_lowers_to(src: &str, expected: &[&str]) {
    let printed = lower_str(src);
    for line in expected {
        assert!(
            printed.lines().any(|printed| printed.trim() == *line),
            "missing `{line}` in\n{printed}"
        );
    }
}

#[test]
fn receivers_are_borrowed_explicitly() {
    assert_lowers_to(
        "class Counter {\n\
         \x20   int count;\n\
         \x20   mut void bump() { this.count++; }\n\
         \x20   int read() { return this.count; }\n\
         \x20   mut int twice() { this.bump(); Counter other = new Counter(); \
         other.bump(); return other.read(); }\n\
         }",
        &[
            "fn Counter.twice(this_0: &mut Counter) -> int {",
            "Counter::bump(this_0);",
            "Counter::bump(&mut other_1);",
            "return Counter::read(&other_1);",
        ],
    );
}

#[test]
fn locals_get_their_own_ids() {
    assert_lowers_to(
        "class A { static int f(int x) {\n\
         \x20   int y = x;\n\
         \x20   for (int i = 0; i < 3; i++) { y += i; }\n\
         \x20   for (int i = 0; i < 3; i++) { y -= i; }\n\
         \x20   return y;\n\
         } }",
        &[
            "fn A.f(x_0: int) -> int {",
            "let y_1: int = x_0;",
            "let i_2: int = 0;",
            "let i_3: int = 0;",
            "while (i_3 < 3) then (i_3++) {",
        ],
    );
}

#[test]
fn varargs_are_packed_into_arrays() {
    assert_lowers_to(
        "class A {\n\
         \x20   static int count(String... parts) { return parts.length; }\n\
         \x20   static int run() { return count(\"a\", \"b\") + count(); }\n\
         }",
        &[
            "return len(parts_0);",
            "return (A::count([\"a\", \"b\"]) + A::count([]));",
        ],
    );
}

#[test]
fn patterns_bind_locals() {
    assert_lowers_to(
        "enum Shape { Circle(double radius), Empty }\n\
         class A { static double area(Shape shape) {\n\
         \x20   return switch (shape) {\n\
         \x20       case Circle(double r) when r > 0.0 -> r * r;\n\
         \x20       case Circle(_), Empty -> 0.0;\n\
         \x20   };\n\
         } }",
        &[
            "Shape::Circle(r_1: double) if (r_1 > 0.0) => (r_1 * r_1)",
            "Shape::Circle(_) | Shape::Empty => 0.0",
        ],
    );
}

#[test]
fn structs_and_variants_are_constructed() {
    assert_lowers_to(
        "record Money(long cents);\n\
         enum Payment { Cash(Money amount), Free }\n\
         class A { static Payment pay() { return Payment.Cash(new Money(5)); } \
         static Payment free() { return Payment.Free; } }",
        &[
            "return Payment::Cash(new Money((5 as long)));",
            "return Payment::Free;",
        ],
    );
}

#[test]
fn implicit_conversions_are_explicit() {
    assert_lowers_to(
        "class A { static long f(int small, byte tiny) {\n\
         \x20   long total = small;\n\
         \x20   total += tiny;\n\
         \x20   double ratio = small + 1.5;\n\
         \x20   return small * 2L;\n\
         } }",
        &[
            "let total_2: long = (small_0 as long);",
            "total_2 += (tiny_1 as long);",
            "let ratio_3: double = ((small_0 as double) + 1.5);",
            "return ((small_0 as long) * 2L);",
        ],
    );
}

#[test]
fn equality_of_objects_calls_equals() {
    assert_lowers_to(
        "record Money(long cents);\n\
         class A { static boolean f(Money a, Money b, String? s) {\n\
         \x20   return a != b || s == null;\n\
         } }",
        &["return (!Equatable::equals(&a_0, &b_1) || (s_2 == null));"],
    );
}

#[test]
fn closures_list_their_captures() {
    assert_lowers_to(
        "import std.util.List;\n\
         class Counter { int count; mut void bump() { this.count++; } }\n\
         class A { static int f(List<int> numbers, Counter counter) {\n\
         \x20   int offset = 1;\n\
         \x20   int calls = 0;\n\
         \x20   numbers.forEach(n -> Console.println(n + offset));\n\
         \x20   numbers.forEach(n -> { calls++; });\n\
         \x20   Runnable bump = counter::bump;\n\
         \x20   return calls;\n\
         } }",
        &[
            "Iterable::forEach(&numbers_0, |n_4: &int| -> void [&offset_2] \
             Console::println((((*n_4) + offset_2) as Object?)));",
            "Iterable::forEach(&numbers_0, |n_5: &int| -> void [&mut calls_3] {",
            "let bump_6: Runnable = ref Counter::bump on &mut counter_1;",
        ],
    );
}

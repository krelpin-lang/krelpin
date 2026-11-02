use std::path::Path;

use krelpin_errors::{DiagCtxt, Diagnostic, DiagnosticBuffer};
use krelpin_parse::{lexer, parser};
use krelpin_span::SourceMap;
use krelpin_sysroot::Sysroot;

fn test_sysroot() -> Sysroot {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    Sysroot::new(root).expect("the workspace is a sysroot")
}

fn diagnostics(src: &str) -> Vec<Diagnostic> {
    let source_map = SourceMap::new();
    let file = source_map.new_source_file("test.kr", src.to_owned());
    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());

    let unit = parser::parse(lexer::lex(&file, &dcx), &dcx);
    let mut units = test_sysroot().load_std(&source_map, &dcx);
    units.push(unit);
    let resolved = krelpin_resolve::resolve(&units, &dcx);
    assert_eq!(
        buffer.messages(),
        Vec::<String>::new(),
        "parse or resolve errors in {src:?}"
    );

    crate::check(&units, &resolved, &dcx);
    buffer.diagnostics()
}

fn errors(src: &str) -> Vec<String> {
    diagnostics(src).into_iter().map(|d| d.message).collect()
}

fn labels(src: &str) -> Vec<String> {
    diagnostics(src)
        .into_iter()
        .filter_map(|d| d.label)
        .collect()
}

fn helps(src: &str) -> Vec<String> {
    diagnostics(src)
        .into_iter()
        .flat_map(|d| d.children.into_iter().map(|child| child.message))
        .collect()
}

fn body(code: &str) -> String {
    format!(
        "class Point {{\n\
         \x20   int x;\n\
         \x20   public int y;\n\
         \x20   Point(int x, int y) {{ this.x = x; this.y = y; }}\n\
         \x20   int sum(&this) {{ return this.x + this.y; }}\n\
         \x20   int scaled(&this, int factor) {{ return this.sum() * factor; }}\n\
         }}\n\
         class Main {{\n\
         \x20   void run(&this) {{ {code} }}\n\
         }}"
    )
}

fn in_body(code: &str) -> Vec<String> {
    errors(&body(code))
}

fn ok(src: &str) {
    assert_eq!(
        errors(src),
        Vec::<String>::new(),
        "unexpected errors for {src:?}"
    );
}

fn body_ok(code: &str) {
    ok(&body(code));
}

#[test]
fn hello_world() {
    ok("class Main { void main() { Console.println(\"Hello World!\"); } }");
}

#[test]
fn literal_and_local_types() {
    body_ok(
        "int a = 1; long b = 2L; double c = 1.5; float d = 1.5f; char e = 'x'; \
             boolean f = true; String g = \"text\"; byte h = 10; long i = a; double j = b;",
    );

    assert_eq!(in_body("int a = \"text\";"), ["mismatched types"]);
    assert_eq!(
        labels(&body("int a = \"text\";")),
        ["expected `int`, found `String`"]
    );
    assert_eq!(in_body("int a = 1L;"), ["mismatched types"]);
    assert_eq!(in_body("boolean b = 1;"), ["mismatched types"]);
}

#[test]
fn null_safety() {
    assert_eq!(in_body("String s = null;"), ["mismatched types"]);
    assert_eq!(
        helps(&body("String s = null;")),
        ["`String` can never be `null`; use `String?` to allow it"]
    );

    body_ok("Point? p = null; int? a = p?.y; int b = p?.sum() ?: 0;");
    assert_eq!(
        in_body("Point? p = null; int a = p.sum();"),
        ["value of type `Point?` may be null"]
    );
    assert_eq!(
        in_body("Point? p = null; int a = p?.sum();"),
        ["mismatched types"]
    );
}

#[test]
fn null_checks_narrow_types() {
    body_ok("Point? p = null; if (p != null) { int a = p.sum(); }");
    body_ok("Point? p = null; if (p == null) { return; } int a = p.sum();");
    body_ok("Point? p = null; boolean big = p != null && p.sum() > 10;");
    body_ok("Point? p = null; boolean empty = p == null || p.sum() == 0;");
    body_ok("Point? p = null; int a = p != null ? p.sum() : 0;");
    body_ok("Point? p = null; if (p == null) { return; } else { int a = p.sum(); }");

    assert_eq!(
        in_body("Point? p = null; if (p != null) { p = null; int a = p.sum(); }"),
        ["value of type `Point?` may be null"]
    );
    assert_eq!(
        in_body("Point? p = null; if (p != null) {} int a = p.sum();"),
        ["value of type `Point?` may be null"]
    );
}

#[test]
fn unnecessary_null_checks_are_warnings() {
    let diagnostics = diagnostics(&body("Point p = new Point(1, 2); int a = p?.sum() ?: 0;"));
    let messages: Vec<String> = diagnostics.iter().map(|d| d.message.clone()).collect();
    assert_eq!(
        messages,
        [
            "unnecessary `?.` on a value that is never null",
            "the left side of `?:` can never be null"
        ]
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| d.level == krelpin_errors::Level::Warning)
    );
}

#[test]
fn fields_and_visibility() {
    body_ok("Point p = new Point(1, 2); int y = p.y;");
    assert_eq!(
        in_body("Point p = new Point(1, 2); int x = p.x;"),
        ["field `x` of `Point` is private"]
    );
    assert_eq!(
        in_body("Point p = new Point(1, 2); int z = p.z;"),
        ["no field `z` on type `Point`"]
    );
    ok("class A { private int secret; int peek(&this, A other) { return other.secret; } }");
}

#[test]
fn private_methods_and_constructors() {
    assert_eq!(
        errors(
            "class A { private void hidden(&this) {} private A() {} }\n\
             class B { void f(&this, A a) { a.hidden(); A b = new A(); } }"
        ),
        [
            "method `hidden` of `A` is private",
            "constructor `A` of `A` is private"
        ]
    );
}

#[test]
fn class_and_instance_members() {
    let src = |code: &str| {
        format!(
            "class Account {{\n\
             \x20   const int LIMIT = 100;\n\
             \x20   int balance;\n\
             \x20   int total(&this) {{ return this.balance; }}\n\
             \x20   int limit() {{ return Account.LIMIT; }}\n\
             \x20   void run(&this) {{ {code} }}\n\
             }}"
        )
    };
    ok(&src(
        "int a = Account.LIMIT; int b = this.balance; int c = Account.limit();",
    ));
    assert_eq!(
        errors(&src("int a = Account.balance;")),
        ["`balance` is a field of each object"]
    );
    assert_eq!(
        errors(&src("int a = this.LIMIT;")),
        ["`LIMIT` belongs to the class `Account`"]
    );
    assert_eq!(
        errors(&src("int a = Account.total();")),
        ["method `total` needs an object to be called on"]
    );
    assert_eq!(
        errors(&src("int a = this.limit();")),
        ["`limit` belongs to the class `Account`"]
    );
}

#[test]
fn method_calls() {
    body_ok("Point p = new Point(1, 2); int a = p.scaled(3);");
    assert_eq!(
        in_body("Point p = new Point(1, 2); int a = p.scaled(1, 2);"),
        ["this method takes 1 argument but 2 arguments were supplied"]
    );
    assert_eq!(
        in_body("Point p = new Point(1, 2); int a = p.scaled(\"x\");"),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("Point p = new Point(1, 2); String a = p.sum();"),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("Point p = new Point(1, 2); p.missing();"),
        ["no method named `missing` found for `Point`"]
    );
}

#[test]
fn overloads() {
    ok("class A {\n\
        \x20   int f(&this, int x) { return x; }\n\
        \x20   String f(&this, String s) { return s; }\n\
        \x20   void run(&this) { int a = this.f(1); String b = this.f(\"x\"); }\n\
        }");
    assert_eq!(
        errors(
            "class A {\n\
             \x20   void f(&this, int x) {}\n\
             \x20   void f(&this, String s) {}\n\
             \x20   void run(&this) { this.f(true); }\n\
             }"
        ),
        ["no overload of `f` accepts arguments of type (boolean)"]
    );
}

#[test]
fn varargs() {
    ok("class A {\n\
        \x20   int sum(int... xs) { return xs.length; }\n\
        \x20   void run(&this) {\n\
        \x20       int a = A.sum();\n\
        \x20       int b = A.sum(1, 2, 3);\n\
        \x20       int[] xs = {1};\n\
        \x20       int c = A.sum(xs);\n\
        \x20   }\n\
        }");
}

#[test]
fn constructors() {
    body_ok("Point p = new Point(1, 2);");
    assert_eq!(
        in_body("Point p = new Point(\"a\", 2);"),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("Point p = new Point(1);"),
        ["this constructor takes 2 arguments but 1 argument was supplied"]
    );
    ok("class A {} class B { void f() { A a = new A(); } }");
    assert_eq!(
        errors("class A {} class B { void f() { A a = new A(1); } }"),
        ["`A` has no constructor that takes 1 argument"]
    );
    assert_eq!(
        errors("abstract class A {} class B { void f() { A a = new A(); } }"),
        ["cannot create an object of an abstract class `A`"]
    );
    assert_eq!(
        errors("interface I {} class B { void f() { I i = new I(); } }"),
        ["cannot create an object of an interface `I`"]
    );
}

#[test]
fn shared_and_const_fields_cannot_be_assigned() {
    let src = |code: &str| {
        format!(
            "class Counter {{\n\
             \x20   const int MAX = 10;\n\
             \x20   shared int count = 0;\n\
             \x20   shared AtomicInt created = new AtomicInt(0);\n\
             \x20   void run(&this) {{ {code} }}\n\
             }}"
        )
    };
    ok(&src(
        "Counter.created.increment(); int a = Counter.count + Counter.MAX;",
    ));
    assert_eq!(
        errors(&src("Counter.count = 1;")),
        ["cannot assign to `shared` field `count`"]
    );
    assert_eq!(
        errors(&src("Counter.count++;")),
        ["cannot assign to `shared` field `count`"]
    );
    assert_eq!(
        errors(&src("Counter.MAX = 2;")),
        ["cannot assign to `const` field `MAX`"]
    );
}

#[test]
fn returns() {
    ok("class A { int f(boolean b) { if (b) { return 1; } else { return 2; } } }");
    ok("class A { int f() { while (true) {} } }");
    ok("class A { int f() throws Exception { throw new Exception(); } }");
    ok("class A { int f() { throw new IllegalStateException(); } }");
    ok("class A { int f(int x) { switch (x) { \
         case 1 -> { return 1; } default -> { return 2; } } } }");
    assert_eq!(
        errors("class A { int f(boolean b) { if (b) { return 1; } } }"),
        ["missing return statement"]
    );
    assert_eq!(
        errors("class A { int f() { while (true) { break; } } }"),
        ["missing return statement"]
    );
    assert_eq!(
        errors("class A { void f() { return 1; } }"),
        ["cannot return a value from a method that returns `void`"]
    );
    assert_eq!(
        errors("class A { int f() { return; } }"),
        ["this method must return a `int`"]
    );
    assert_eq!(
        errors("class A { int f() { return \"x\"; } }"),
        ["mismatched types"]
    );
}

#[test]
fn operators() {
    body_ok(
        "String s = \"n=\" + 1; boolean b = 1 < 2.5; long l = 1 + 2L; int m = 7 % 3; \
             boolean c = true & false; int bits = 1 << 3;",
    );
    assert_eq!(
        in_body("int a = true + 1;"),
        ["cannot apply `+` to `boolean` and `int`"]
    );
    assert_eq!(
        in_body("boolean b = 1 < \"a\";"),
        ["cannot apply `<` to `int` and `String`"]
    );
    assert_eq!(in_body("boolean b = !5;"), ["cannot apply `!` to `int`"]);
    assert_eq!(in_body("if (1) {}"), ["mismatched types"]);
    assert_eq!(
        in_body("boolean b = 1 == true;"),
        ["cannot compare `int` with `boolean`"]
    );
}

#[test]
fn switch_expressions() {
    body_ok("int x = 1; String s = switch (x) { case 1 -> \"a\"; default -> { yield \"b\"; } };");
    assert_eq!(
        in_body("int x = 1; Object s = switch (x) { case 1 -> \"a\"; default -> 2; };"),
        ["the cases of this `switch` have incompatible types `String` and `int`"]
    );
    assert_eq!(
        in_body("int x = 1; String s = switch (x) { case 1 -> \"a\"; };"),
        ["this `switch` expression does not cover every case"]
    );

    let enums = |code: &str| {
        format!(
            "enum Color {{ RED, GREEN }}\n\
             class A {{ String f(Color c) {{ return {code}; }} }}"
        )
    };
    ok(&enums(
        "switch (c) { case RED -> \"r\"; case GREEN -> \"g\"; }",
    ));
    assert_eq!(
        errors(&enums("switch (c) { case RED -> \"r\"; }")),
        ["this `switch` expression does not cover every case"]
    );
    assert_eq!(
        errors(&enums(
            "switch (c) { case BLUE -> \"b\"; default -> \"?\"; }"
        )),
        ["no constant `BLUE` in enum `Color`"]
    );
}

#[test]
fn inheritance_and_generics() {
    let src = |code: &str| {
        format!(
            "class Animal {{ String name(&this) {{ return \"animal\"; }} }}\n\
             class Dog extends Animal {{}}\n\
             class Box<T> {{\n\
             \x20   T value;\n\
             \x20   Box(T value) {{ this.value = value; }}\n\
             \x20   T get(&this) {{ return this.value; }}\n\
             }}\n\
             class Main {{ void run(&this) {{ {code} }} }}"
        )
    };
    ok(&src(
        "Dog d = new Dog(); String n = d.name(); Animal a = new Dog();",
    ));
    assert_eq!(errors(&src("Dog d = new Animal();")), ["mismatched types"]);
    ok(&src(
        "Box<String> b = new Box<String>(\"x\"); String s = b.get();",
    ));
    assert_eq!(
        errors(&src(
            "Box<String> b = new Box<String>(\"x\"); int n = b.get();"
        )),
        ["mismatched types"]
    );
    assert_eq!(
        labels(&src(
            "Box<String> b = new Box<String>(\"x\"); int n = b.get();"
        )),
        ["expected `int`, found `String`"]
    );
}

#[test]
fn cyclic_inheritance() {
    assert_eq!(
        errors("class A extends B {} class B extends A {}"),
        ["cyclic inheritance: `A` inherits from itself"]
    );
}

#[test]
fn references() {
    body_ok("String s = \"x\"; &String r = &s; &mut String m = &mut s; &String fromMut = m;");
    assert_eq!(
        in_body("String s = \"x\"; &String r = s;"),
        ["mismatched types"]
    );
    assert_eq!(
        helps(&body("String s = \"x\"; &String r = s;")),
        ["borrow the value with `&`"]
    );
    assert_eq!(
        in_body("String s = \"x\"; &mut String m = &s;"),
        ["mismatched types"]
    );
}

#[test]
fn arrays() {
    body_ok(
        "int[] xs = {1, 2}; int x = xs[0]; int n = xs.length; int[] ys = new int[n]; \
             for (int v : xs) { int w = v; }",
    );
    assert_eq!(in_body("int[] xs = {\"a\"};"), ["mismatched types"]);
    assert_eq!(
        in_body("int[] xs = {1}; int x = xs[\"0\"];"),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("int[] xs = {1}; for (String s : xs) {}"),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("int x = 1; int y = x[0];"),
        ["cannot index into a value of type `int`"]
    );
}

#[test]
fn casts() {
    body_ok("double d = 1.5; int i = (int) d; long l = (long) i;");
    assert_eq!(
        in_body("int i = (int) true;"),
        ["cannot cast `boolean` to `int`"]
    );
}

#[test]
fn types_from_unknown_packages_are_lenient() {
    ok("import shop.money.Price;\n\
        class A { int f(Price price) { return price.cents() + price.anything; } }");
}

#[test]
fn standard_library_has_no_errors() {
    ok("");
}

#[test]
fn string_methods_are_checked() {
    body_ok(
        "String s = \"Krelpin\"; int n = s.length(); char c = s.chars()[0]; \
         boolean b = s.contains(\"pin\") && !s.isEmpty(); String[] parts = s.split(\",\"); \
         String joined = String.join(\", \", \"a\", \"b\");",
    );
    assert_eq!(
        in_body("String s = \"x\"; int n = s.lenght();"),
        ["no method named `lenght` found for `String`"]
    );
    assert_eq!(
        in_body("String s = \"x\"; String n = s.length();"),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("String s = \"x\"; String part = s.substring(\"0\");"),
        ["mismatched types"]
    );
}

#[test]
fn every_object_has_the_methods_of_object() {
    body_ok("Point p = new Point(1, 2); String text = p.toString(); Object any = p;");
    ok("interface Shape { double area(&this); }\n\
        class Main { String f(Shape shape) { return shape.toString(); } }");
    ok("enum Color { RED, GREEN }\n\
        class Main { String f(Color c) { return c.name() + c.ordinal(); } }");
}

#[test]
fn collections_are_generic() {
    body_ok(
        "List<String> names = new ArrayList<>(); names.add(\"x\"); int n = names.size(); \
         String first = names.get(0); \
         for (String name : names) { Console.println(name.length()); }",
    );
    body_ok(
        "Map<String, int> ages = new HashMap<>(); ages.put(\"Ali\", 30); \
         int? age = ages.get(\"Ali\"); int years = age ?: 0;",
    );
    assert_eq!(
        in_body("List<String> names = new ArrayList<>(); int first = names.get(0);"),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("List<String> names = new ArrayList<>(); names.add(1);"),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("List<String> names = new ArrayList<>(); for (int name : names) {}"),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("Map<String, int> ages = new HashMap<>(); int age = ages.get(\"Ali\");"),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("Map<String, int> ages = new HashMap<>(); for (String key : ages) {}"),
        ["cannot iterate over a value of type `Map<String, int>`"]
    );
    assert_eq!(
        in_body("List<String> names = new ArrayList<>(); String s = names[0];"),
        ["cannot index into a value of type `List<String>`"]
    );
    assert_eq!(
        helps(&body(
            "List<String> names = new ArrayList<>(); String s = names[0];"
        )),
        ["use `.get(index)` instead"]
    );
}

#[test]
fn own_types_can_be_iterable() {
    ok("abstract class Countdown implements Iterable<int> {}\n\
        class Main {\n\
        \x20   void run(&this, Countdown countdown) {\n\
        \x20       for (int n : countdown) { int x = n; }\n\
        \x20   }\n\
        }");
}

#[test]
fn primitives_are_the_only_number_types() {
    body_ok(
        "List<int> xs = new ArrayList<>(); xs.add(5); int first = xs.get(0); \
         Map<String, int> ages = new HashMap<>(); int? age = ages.get(\"Ali\"); \
         int? missing = null; Object any = 5; Comparable<int> order = 5;",
    );
    assert_eq!(
        errors("import std.primitive.Int; class A { Int x; }"),
        ["`Int` is not a type"]
    );
    assert_eq!(
        helps("import std.primitive.Int; class A { Int x; }"),
        ["use `int` instead"]
    );
    assert_eq!(
        in_body("int? maybe = null; int x = maybe;"),
        ["mismatched types"]
    );
}

#[test]
fn primitives_have_members() {
    body_ok(
        "int big = int.MAX_VALUE; long small = long.MIN_VALUE; int parsed = int.parse(\"42\"); \
         int order = 5.compareTo(&6); String text = 5.toString(); boolean digit = 'x'.isDigit();",
    );
    assert_eq!(
        in_body("int x = 5; int n = x.lenght();"),
        ["no method named `lenght` found for `int`"]
    );
    assert_eq!(in_body("int n = int.parse(5);"), ["mismatched types"]);
    assert_eq!(
        in_body("int n = 5.parse(\"5\");"),
        ["`parse` belongs to the class `int`"]
    );
}

#[test]
fn class_members_of_the_standard_library() {
    body_ok(
        "int a = Math.max(1, 2); long b = Math.max(1L, 2); double c = Math.sqrt(2.0) * Math.PI; \
         int big = int.MAX_VALUE; int parsed = int.parse(\"42\");",
    );
    assert_eq!(in_body("int a = Math.sqrt(2.0);"), ["mismatched types"]);
    assert_eq!(
        in_body("Console c = new Console();"),
        ["constructor `Console` of `Console` is private"]
    );
    assert_eq!(
        in_body("Math.PI = 3.0;"),
        ["cannot assign to `const` field `PI`"]
    );
}

#[test]
fn console_prints_values_and_borrows() {
    body_ok(
        "Console.println(); Console.println(1); Console.println(2L); Console.println('c'); \
         Console.println(true); Console.println(1.5); Console.println(\"text\"); \
         Console.println(new Point(1, 2)); String s = \"x\"; Console.println(&s); \
         String? maybe = null; Console.print(maybe); String? line = Console.readLine();",
    );
    assert_eq!(
        in_body("String line = Console.readLine();"),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("Console.print(null);"),
        ["call to `print` is ambiguous"]
    );
}

#[test]
fn exceptions() {
    body_ok(
        "try { throw new IllegalArgumentException(\"bad\"); } \
         catch (RuntimeException e) { String? message = e.getMessage(); }",
    );
    assert_eq!(
        in_body("try {} catch (Exception e) { String message = e.getMessage(); }"),
        ["mismatched types"]
    );
    ok("import std.io.IOException;\n\
        class Main { void run(&this) throws IOException { throw new IOException(\"disk\"); } }");
}

#[test]
fn atomics_can_change_through_shared_fields() {
    ok("class Counter {\n\
        \x20   shared AtomicInt created = new AtomicInt(0);\n\
        \x20   Counter() { int id = Counter.created.increment(); }\n\
        }");
    assert_eq!(
        errors("class Counter { shared AtomicInt created = new AtomicInt(\"0\"); }"),
        ["mismatched types"]
    );
}

#[test]
fn generic_arguments_are_counted() {
    assert_eq!(
        errors("class A { List<String, int> xs; }"),
        ["`List` takes 1 type argument but 2 were supplied"]
    );
    assert_eq!(
        errors("class A { String<int> s; }"),
        ["`String` takes 0 type arguments but 1 was supplied"]
    );
    assert_eq!(
        errors("class A<T> { T<String> x; }"),
        ["type parameter `T` does not take type arguments"]
    );
    assert_eq!(
        in_body("Map<String>? ages = null;"),
        ["`Map` takes 2 type arguments but 1 was supplied"]
    );
}

#[test]
fn generic_arguments_satisfy_bounds() {
    let src = |code: &str| {
        format!(
            "class Sorted<T extends Comparable<T>> {{}}\n\
             class Thing {{}}\n\
             class Main {{ void run(&this) {{ {code} }} }}"
        )
    };
    ok(&src(
        "Sorted<String>? s = null; Sorted<int>? i = new Sorted<>();",
    ));
    assert_eq!(
        errors(&src("Sorted<Thing>? s = null;")),
        ["type `Thing` does not satisfy the bound of `T`"]
    );
    assert_eq!(
        labels(&src("Sorted<Thing>? s = null;")),
        ["`Thing` is not a `Comparable<Thing>`"]
    );
}

#[test]
fn type_parameters_have_the_methods_of_their_bounds() {
    ok("class Max {\n\
        \x20   <T extends Comparable<T>> T max(T a, T b) {\n\
        \x20       return a.compareTo(&b) >= 0 ? a : b;\n\
        \x20   }\n\
        }");
    ok("class Show<T> { String show(&this, T value) { return value.toString(); } }");
    ok("class View<T extends Comparable<T>> {\n\
        \x20   Comparable<T> view(&this, T value) { return value; }\n\
        }");
    assert_eq!(
        errors("class Show<T> { int size(&this, T value) { return value.length(); } }"),
        ["no method named `length` found for `T`"]
    );
    assert_eq!(
        errors("class View<T> { Comparable<T> view(&this, T value) { return value; } }"),
        ["mismatched types"]
    );
}

fn pick(code: &str) -> String {
    format!(
        "class Pick {{\n\
         \x20   int f(long x) {{ return 1; }}\n\
         \x20   String f(int x) {{ return \"\"; }}\n\
         \x20   boolean g(Object? x) {{ return true; }}\n\
         \x20   char g(String? x) {{ return 'c'; }}\n\
         \x20   int v(String... xs) {{ return 0; }}\n\
         \x20   String v(String x) {{ return x; }}\n\
         \x20   void amb(int a, long b) {{}}\n\
         \x20   void amb(long a, int b) {{}}\n\
         }}\n\
         class Main {{ void run(&this) {{ {code} }} }}"
    )
}

#[test]
fn the_most_specific_overload_is_chosen() {
    ok(&pick(
        "String a = Pick.f(1); int b = Pick.f(1L); char c = Pick.g(\"x\"); \
         boolean d = Pick.g(new Pick());",
    ));
    assert_eq!(errors(&pick("int a = Pick.f(1);")), ["mismatched types"]);
    assert_eq!(
        errors(&pick("char d = Pick.g(new Pick());")),
        ["mismatched types"]
    );
}

#[test]
fn variadic_methods_are_the_last_resort() {
    ok(&pick(
        "String a = Pick.v(\"x\"); int b = Pick.v(\"x\", \"y\"); int c = Pick.v();",
    ));
}

#[test]
fn ambiguous_calls() {
    assert_eq!(
        errors(&pick("Pick.amb(1, 2);")),
        ["call to `amb` is ambiguous"]
    );
    assert_eq!(
        helps(&pick("Pick.amb(1, 2);")),
        [
            "it could call `Pick.amb(int, long)` or `Pick.amb(long, int)`",
            "cast the arguments to the types of the method you want to call",
        ]
    );
}

fn util(code: &str) -> String {
    format!(
        "class Util {{\n\
         \x20   <T> T first(List<T> xs) {{ return xs.get(0); }}\n\
         \x20   <T> T either(T a, T b) {{ return a; }}\n\
         \x20   <T extends Comparable<T>> T max(T a, T b) {{\n\
         \x20       return a.compareTo(&b) >= 0 ? a : b;\n\
         \x20   }}\n\
         \x20   <T> List<T> listOf(T... xs) {{ return new ArrayList<>(); }}\n\
         }}\n\
         class Thing {{}}\n\
         class Main {{ void run(&this) {{ {code} }} }}"
    )
}

#[test]
fn generic_methods_infer_their_type_arguments() {
    ok(&util(
        "List<String> names = new ArrayList<>(); String s = Util.first(names); \
         int i = Util.either(1, 2); String m = Util.max(\"a\", \"b\"); \
         List<String> xs = Util.listOf(\"a\", \"b\");",
    ));
    assert_eq!(
        errors(&util(
            "List<String> names = new ArrayList<>(); int n = Util.first(names);"
        )),
        ["mismatched types"]
    );
    assert_eq!(
        labels(&util("List<int> xs = Util.listOf(\"a\");")),
        ["expected `List<int>`, found `List<String>`"]
    );
    assert_eq!(
        errors(&util("Object o = Util.either(\"a\", 1);")),
        ["mismatched types"]
    );
}

#[test]
fn inferred_type_arguments_satisfy_bounds() {
    assert_eq!(
        errors(&util("Thing t = Util.max(new Thing(), new Thing());")),
        ["type `Thing` does not satisfy the bound of `T`"]
    );
}

fn closures(code: &str) -> String {
    format!(
        "import std.function.Consumer;\n\
         interface Mapper<A, B> {{ B apply(&this, A value); }}\n\
         class Thing {{ Thing(String name) {{}} }}\n\
         class Exec {{\n\
         \x20   int run(Runnable task) {{ return 0; }}\n\
         \x20   String run(Consumer<String> action) {{ return \"\"; }}\n\
         }}\n\
         class Main {{\n\
         \x20   int helper(&this, String s) {{ return 1; }}\n\
         \x20   void run(&this) {{ {code} }}\n\
         }}"
    )
}

#[test]
fn lambdas_take_the_types_of_their_target() {
    ok(&closures(
        "Runnable r = () -> Console.println(\"x\"); \
         Consumer<String> c = s -> Console.println(s.length()); \
         Mapper<String, int> m = s -> s.length(); \
         Mapper<String, int> b = (String s) -> { return s.length(); }; \
         List<String> names = new ArrayList<>(); \
         names.forEach(name -> Console.println(name.length()));",
    ));
    assert_eq!(
        errors(&closures("Mapper<String, int> m = s -> s.lenght();")),
        ["no method named `lenght` found for `String`"]
    );
    assert_eq!(
        errors(&closures(
            "List<String> names = new ArrayList<>(); names.forEach(name -> name.lenght());"
        )),
        ["no method named `lenght` found for `String`"]
    );
    assert_eq!(
        labels(&closures("Mapper<String, int> m = s -> s;")),
        ["expected `int`, found `String`"]
    );
}

#[test]
fn lambda_errors() {
    assert_eq!(
        errors(&closures("Mapper<String, int> m = (a, b) -> 1;")),
        ["this lambda takes 2 parameters but `apply` takes 1 parameter"]
    );
    assert_eq!(
        labels(&closures("Mapper<String, int> m = (int s) -> 1;")),
        ["expected `String`, found `int`"]
    );
    assert_eq!(
        errors(&closures(
            "Mapper<String, int> m = s -> { Console.println(s); };"
        )),
        ["missing return statement"]
    );
    assert_eq!(
        errors(&closures("String s = () -> 1;")),
        ["a lambda cannot be used as `String`"]
    );
}

#[test]
fn lambdas_choose_overloads_by_their_parameters() {
    ok(&closures(
        "int a = Exec.run(() -> {}); String b = Exec.run(s -> Console.println(s.length()));",
    ));
}

#[test]
fn method_references() {
    ok(&closures(
        "Mapper<String, int> len = String::length; \
         Mapper<String, int> parse = int::parse; \
         Consumer<String> print = Console::println; \
         Mapper<String, Thing> make = Thing::new; \
         Mapper<String, int> own = this::helper;",
    ));
    assert_eq!(
        errors(&closures("Mapper<String, int> size = String::size;")),
        ["`String::size` does not match `Mapper<String, int>`"]
    );
    assert_eq!(
        labels(&closures("Mapper<String, String> len = String::length;")),
        ["expected `String`, found `int`"]
    );
}

fn shapes(code: &str) -> String {
    format!(
        "interface Shape {{\n\
         \x20   double area(&this);\n\
         \x20   String name(&this) {{ return \"shape\"; }}\n\
         }}\n\
         interface Box<T> {{ T get(&this); }}\n\
         {code}"
    )
}

#[test]
fn abstract_methods_are_implemented() {
    ok(&shapes(
        "class Circle implements Shape { double area(&this) { return 3.14; } }",
    ));
    ok(&shapes("abstract class Base implements Shape {}"));
    ok(&shapes(
        "class Base { double area(&this) { return 1.0; } }\n\
         class Square extends Base implements Shape {}",
    ));
    ok(&shapes(
        "class IntBox implements Box<int> { int get(&this) { return 1; } }",
    ));

    let src = shapes("class Square implements Shape {}");
    assert_eq!(
        errors(&src),
        ["`Square` does not implement every method of `Shape`"]
    );
    assert_eq!(
        helps(&src),
        [
            "missing: `double area(&this)`",
            "implement the missing methods, or mark `Square` as `abstract`",
        ]
    );
    assert_eq!(
        helps(&shapes("struct Point implements Shape {}")),
        [
            "missing: `double area(&this)`",
            "implement the missing methods"
        ]
    );
    assert_eq!(
        errors(&shapes("enum Kind implements Shape { ROUND }")),
        ["`Kind` does not implement every method of `Shape`"]
    );
    assert_eq!(
        errors("class A { abstract void f(&this); }"),
        ["`A` is not abstract but declares abstract methods"]
    );
}

fn base(code: &str) -> String {
    format!(
        "class Base {{\n\
         \x20   void f(&this) {{}}\n\
         \x20   final void g(&this) {{}}\n\
         \x20   public void h(&this) {{}}\n\
         \x20   Base copy(&this) {{ return new Base(); }}\n\
         }}\n\
         {code}"
    )
}

#[test]
fn overrides_keep_the_signature() {
    ok(&base(
        "class A extends Base {\n\
         \x20   void f(&this) {}\n\
         \x20   void f(&mut this, int x) {}\n\
         \x20   A copy(&this) { return new A(); }\n\
         \x20   String toString(&this) { return \"a\"; }\n\
         }",
    ));
    assert_eq!(
        errors(&base("class A extends Base { void f(&mut this) {} }")),
        ["`f` takes `&mut this` but `Base.f` takes `&this`"]
    );
    assert_eq!(
        errors(&base("class A extends Base { void f() {} }")),
        ["`f` takes no `this` but `Base.f` takes `&this`"]
    );
    assert_eq!(
        errors(&base("class A extends Base { void g(&this) {} }")),
        ["cannot override `Base.g` because it is `final`"]
    );
    assert_eq!(
        errors(&base("class A extends Base { private void h(&this) {} }")),
        ["`h` cannot be less visible than `Base.h`"]
    );
    assert_eq!(
        errors(&base("class A extends Base { int f(&this) { return 1; } }")),
        ["`f` returns `int` but `Base.f` returns `void`"]
    );
    assert_eq!(
        errors(&base(
            "class A extends Base { String copy(&this) { return \"\"; } }"
        )),
        ["`copy` returns `String` but `Base.copy` returns `Base`"]
    );
    assert_eq!(
        errors("class A { int toString(&this) { return 1; } }"),
        ["`toString` returns `int` but `Object.toString` returns `String`"]
    );
    assert_eq!(
        errors(&shapes(
            "class StrBox implements Box<String> { int get(&this) { return 1; } }"
        )),
        ["`get` returns `int` but `Box.get` returns `String`"]
    );
}

#[test]
fn parents_have_the_right_kind() {
    assert_eq!(
        errors("interface I {} class A extends I {}"),
        ["a class cannot extend an interface"]
    );
    assert_eq!(
        helps("interface I {} class A extends I {}"),
        ["use `implements I` instead"]
    );
    assert_eq!(
        errors("class B {} class A implements B {}"),
        ["`B` is not an interface"]
    );
    assert_eq!(
        helps("class B {} class A implements B {}"),
        ["use `extends B` instead"]
    );
    assert_eq!(
        errors("class B {} interface I extends B {}"),
        ["an interface can only extend interfaces"]
    );
    assert_eq!(
        errors("struct S {} class A extends S {}"),
        ["a class can only extend another class"]
    );
    assert_eq!(
        errors("final class F {} class A extends F {}"),
        ["cannot extend `F` because it is `final`"]
    );
    assert_eq!(
        errors("class A extends String {}"),
        ["cannot extend `String` because it is `final`"]
    );
    assert_eq!(
        errors("interface I { void f(&this); } class A extends I {}"),
        ["a class cannot extend an interface"]
    );
}

#[test]
fn only_throwables_are_thrown_and_caught() {
    body_ok("throw new IllegalStateException(\"broken\");");
    assert_eq!(
        in_body("throw \"oops\";"),
        ["cannot throw a value of type `String`"]
    );
    assert_eq!(
        in_body("RuntimeException? error = null; throw error;"),
        ["cannot throw a value of type `RuntimeException?`"]
    );
    assert_eq!(
        errors("class A { void f() throws String {} }"),
        ["`String` is not a `Throwable`"]
    );
    assert_eq!(
        in_body("try {} catch (String s) {}"),
        ["`String` cannot be caught"]
    );
}

#[test]
fn multi_catch_binds_the_common_superclass() {
    body_ok(
        "try {} catch (IllegalArgumentException | IllegalStateException e) { \
         RuntimeException r = e; String? message = e.getMessage(); }",
    );
    assert_eq!(
        in_body(
            "try {} catch (IllegalArgumentException | IllegalStateException e) { \
             IllegalStateException s = e; }"
        ),
        ["mismatched types"]
    );
    assert_eq!(
        in_body("try {} catch (RuntimeException | IllegalStateException e) {}"),
        ["`IllegalStateException` and `RuntimeException` overlap in this `catch`"]
    );
}

#[test]
fn exceptions_are_caught_only_once() {
    body_ok("try {} catch (RuntimeException e) {} catch (Exception e) {}");
    let code = "try {} catch (Exception e) {} catch (RuntimeException e) {}";
    assert_eq!(
        in_body(code),
        ["`RuntimeException` has already been caught"]
    );
    assert_eq!(
        labels(&body(code)),
        ["already caught by the `catch` for `Exception`"]
    );
}

fn files(code: &str) -> String {
    format!(
        "import std.io.IOException;\n\
         class Files {{\n\
         \x20   String read(String path) throws IOException {{ return path; }}\n\
         \x20   void write(String text) {{}}\n\
         }}\n\
         interface Source {{ String next(&this) throws IOException; }}\n\
         class Main {{\n\
         \x20   {code}\n\
         }}"
    )
}

#[test]
fn checked_exceptions_are_caught_or_declared() {
    ok(&files(
        "void run(&this) throws IOException { String text = Files.read(\"a\"); }",
    ));
    ok(&files(
        "void run(&this) throws Exception { Files.read(\"a\"); }",
    ));
    ok(&files(
        "void run(&this) { try { Files.read(\"a\"); } catch (IOException e) {} }",
    ));
    ok(&files(
        "void run(&this) { try { Files.read(\"a\"); } catch (Exception e) {} }",
    ));
    ok(&files(
        "void run(&this) { throw new IllegalArgumentException(\"bad\"); }",
    ));

    let src = files("void run(&this) { Files.read(\"a\"); }");
    assert_eq!(errors(&src), ["unreported exception `IOException`"]);
    assert_eq!(
        helps(&src),
        [
            "checked exceptions must be caught or declared with `throws`",
            "catch it with `try`, or add `throws IOException` to the method",
        ]
    );
    assert_eq!(
        errors(&files(
            "void run(&this) { throw new IOException(\"disk\"); }"
        )),
        ["unreported exception `IOException`"]
    );
    assert_eq!(
        errors(&files(
            "void run(&this) { try { Files.read(\"a\"); } catch (RuntimeException e) {} }"
        )),
        ["unreported exception `IOException`"]
    );
}

#[test]
fn exceptions_thrown_in_catch_blocks_are_not_caught_by_them() {
    assert_eq!(
        errors(&files(
            "void run(&this) { \
             try { Files.read(\"a\"); } catch (IOException e) { Files.read(\"b\"); } }"
        )),
        ["unreported exception `IOException`"]
    );
    ok(&files(
        "void run(&this) { try { try { Files.read(\"a\"); } finally {} } \
         catch (IOException e) {} }",
    ));
}

#[test]
fn lambdas_cannot_throw_checked_exceptions_their_method_does_not() {
    let src = files(
        "void run(&this) throws IOException { \
         List<String> paths = new ArrayList<>(); \
         paths.forEach(path -> Files.read(path)); }",
    );
    assert_eq!(errors(&src), ["unreported exception `IOException`"]);
    assert_eq!(
        helps(&src)[1],
        "catch it inside the lambda, because `accept` cannot throw it"
    );
    ok(&files(
        "void run(&this) { Source s = () -> Files.read(\"a\"); }",
    ));
}

#[test]
fn constructors_declare_their_exceptions() {
    ok("import std.io.IOException;\n\
        class Conn { Conn() throws IOException {} }\n\
        class Main { void run(&this) throws IOException { Conn c = new Conn(); } }");
    assert_eq!(
        errors(
            "import std.io.IOException;\n\
             class Conn { Conn() throws IOException {} }\n\
             class Main { void run(&this) { Conn c = new Conn(); } }"
        ),
        ["unreported exception `IOException`"]
    );
}

#[test]
fn catching_checked_exceptions_that_are_never_thrown() {
    assert_eq!(
        errors(&files(
            "void run(&this) { try { Files.write(\"a\"); } catch (IOException e) {} }"
        )),
        ["`IOException` is never thrown in the body of this `try`"]
    );
    ok(&files(
        "void run(&this) { try { Files.write(\"a\"); } catch (Exception e) {} }",
    ));
    ok(&files(
        "void run(&this) { try { Files.write(\"a\"); } catch (IllegalStateException e) {} }",
    ));
    ok("import std.io.IOException;\n\
        import java.net.Socket;\n\
        class Main {\n\
        \x20   void run(&this, Socket s) { try { s.connect(); } catch (IOException e) {} }\n\
        }");
}

#[test]
fn overrides_cannot_throw_more() {
    let src = |code: &str| {
        format!(
            "import std.io.IOException;\n\
             class Base {{ void f(&this) {{}} void g(&this) throws Exception {{}} }}\n\
             {code}"
        )
    };
    ok(&src(
        "class A extends Base { void g(&this) throws IOException {} \
         void f(&this) throws IllegalStateException {} }",
    ));
    assert_eq!(
        errors(&src(
            "class A extends Base { void f(&this) throws IOException {} }"
        )),
        ["`f` cannot throw `IOException` because `Base.f` does not"]
    );
}

fn bag(code: &str) -> String {
    format!(
        "class Bag<T> {{\n\
         \x20   T first;\n\
         \x20   Bag(T first) {{ this.first = first; }}\n\
         \x20   boolean isSmall(&this, T limit) where T extends Comparable<T> {{\n\
         \x20       return this.first.compareTo(&limit) < 0;\n\
         \x20   }}\n\
         }}\n\
         class Thing {{}}\n\
         class Main {{ void run(&this) {{ {code} }} }}"
    )
}

#[test]
fn where_clauses_are_checked_at_the_call() {
    ok(&bag(
        "Bag<String> words = new Bag<String>(\"a\"); boolean small = words.isSmall(\"b\");",
    ));
    ok(&bag("Bag<Thing> things = new Bag<Thing>(new Thing());"));
    assert_eq!(
        errors(&bag(
            "Bag<Thing> things = new Bag<Thing>(new Thing()); things.isSmall(new Thing());"
        )),
        ["type `Thing` does not satisfy the bound of `T`"]
    );
    assert_eq!(
        errors(
            "class Bag<T> { T first; boolean bad(&this, T other) { \
             return this.first.compareTo(&other) < 0; } }"
        ),
        ["no method named `compareTo` found for `T`"]
    );
}

fn points(code: &str) -> String {
    format!(
        "class Point implements Hashable<Point> {{\n\
         \x20   int x;\n\
         \x20   Point(int x) {{ this.x = x; }}\n\
         \x20   boolean equals(&this, &Point other) {{ return this.x == other.x; }}\n\
         \x20   int hashCode(&this) {{ return this.x; }}\n\
         }}\n\
         class Thing {{}}\n\
         enum Color {{ RED, GREEN }}\n\
         class Main {{ void run(&this) {{ {code} }} }}"
    )
}

#[test]
fn equality_needs_equatable() {
    ok(&points(
        "Point a = new Point(1); Point b = new Point(2); boolean same = a == b; \
         boolean text = \"a\" != \"b\"; boolean number = 1 == 1L; \
         boolean color = Color.RED == Color.GREEN; Point? maybe = null; \
         boolean missing = maybe == null; boolean found = maybe == a;",
    ));
    let src = points("Thing a = new Thing(); boolean same = a == new Thing();");
    assert_eq!(
        errors(&src),
        ["values of type `Thing` cannot be compared with `==`"]
    );
    assert_eq!(helps(&src), ["implement `Equatable<Thing>` for `Thing`"]);
    assert_eq!(
        errors(&points("boolean wrong = \"a\" == 1;")),
        ["cannot compare `String` with `int`"]
    );
    assert_eq!(
        errors(&points("int x = 1; boolean wrong = x == null;")),
        ["cannot compare `int` with `null`"]
    );
}

#[test]
fn generic_equality_needs_a_bound() {
    ok("class Box<T extends Equatable<T>> { boolean same(&this, T a, T b) { return a == b; } }");
    let src = "class Box<T> { boolean same(&this, T a, T b) { return a == b; } }";
    assert_eq!(
        errors(src),
        ["values of type `T` cannot be compared with `==`"]
    );
    assert_eq!(helps(src), ["add the bound `T extends Equatable<T>`"]);
}

#[test]
fn hash_collections_need_hashable_keys() {
    ok(&points(
        "Map<Point, String> names = new HashMap<Point, String>(); \
         Set<Color> colors = new HashSet<Color>(); Set<String> words = new HashSet<String>();",
    ));
    assert_eq!(
        errors(&points(
            "Map<Thing, int> counts = new HashMap<Thing, int>();"
        )),
        ["type `Thing` does not satisfy the bound of `K`"]
    );
}

#[test]
fn list_lookups_need_equatable_elements() {
    ok(&points(
        "List<Point> points = new ArrayList<>(); boolean has = points.contains(new Point(1));",
    ));
    assert_eq!(
        errors(&points(
            "List<Thing> things = new ArrayList<>(); things.contains(new Thing());"
        )),
        ["type `Thing` does not satisfy the bound of `T`"]
    );
}

#[test]
fn chars_are_unicode_scalars() {
    body_ok(
        "char smile = '😀'; char last = char.MAX_VALUE; int code = smile; \
         char[] letters = \"Ayşe 😀\".chars(); boolean letter = letters[2].isLetter();",
    );
    assert_eq!(
        in_body("String s = \"x\"; char c = s.charAt(0);"),
        ["no method named `charAt` found for `String`"]
    );
}

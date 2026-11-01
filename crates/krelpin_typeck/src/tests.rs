use krelpin_errors::{DiagCtxt, Diagnostic, DiagnosticBuffer};
use krelpin_parse::{lexer, parser};
use krelpin_span::SourceMap;

fn diagnostics(src: &str) -> Vec<Diagnostic> {
    let source_map = SourceMap::new();
    let file = source_map.new_source_file("test.kr", src.to_owned());
    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());

    let unit = parser::parse(lexer::lex(&file, &dcx), &dcx);
    let resolved = krelpin_resolve::resolve(&unit, &dcx);
    assert_eq!(
        buffer.messages(),
        Vec::<String>::new(),
        "parse or resolve errors in {src:?}"
    );

    crate::check(&unit, &resolved.resolutions, &dcx);
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
fn var_inference() {
    body_ok("var a = 5; int b = a; var p = new Point(1, 2); int s = p.sum();");
    assert_eq!(
        in_body("var a = null;"),
        ["cannot infer the type of `a` from `null`"]
    );
    assert_eq!(in_body("var a;"), ["`var` needs an initializer"]);
    assert_eq!(in_body("var a = 5; String b = a;"), ["mismatched types"]);
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
    ok("class A { int f() { throw new Exception(); } }");
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
        in_body("int x = 1; var s = switch (x) { case 1 -> \"a\"; default -> 2; };"),
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
fn unknown_library_types_are_lenient() {
    body_ok(
        "List<String> names = new ArrayList<>(); names.add(\"x\"); int n = names.size(); \
             for (String name : names) { Console.println(name.length()); }",
    );
}

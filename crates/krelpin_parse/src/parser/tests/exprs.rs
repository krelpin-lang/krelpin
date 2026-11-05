use super::{body, expr};
use krelpin_ast::ast::StmtKind;

#[test]
fn arithmetic_precedence() {
    assert_eq!(expr("1 + 2 * 3"), "(1 + (2 * 3))");
    assert_eq!(expr("(1 + 2) * 3"), "((1 + 2) * 3)");
    assert_eq!(expr("a - b - c"), "((a - b) - c)");
    assert_eq!(expr("a << 1 + 2"), "(a << (1 + 2))");
    assert_eq!(expr("a % b / c"), "((a % b) / c)");
}

#[test]
fn logical_and_bitwise_precedence() {
    assert_eq!(expr("a || b && c"), "(a || (b && c))");
    assert_eq!(expr("a & b == c"), "(a & (b == c))");
    assert_eq!(expr("a == b < c"), "(a == (b < c))");
    assert_eq!(expr("a | b ^ c & d"), "(a | (b ^ (c & d)))");
}

#[test]
fn elvis_binds_tighter_than_comparison() {
    assert_eq!(expr("name?.length() ?: 0"), "(name?.length() ?: 0)");
    assert_eq!(expr("a ?: b > c"), "((a ?: b) > c)");
    assert_eq!(expr("x ?: 0 + 1"), "(x ?: (0 + 1))");
}

#[test]
fn ternary_is_right_associative() {
    assert_eq!(expr("a ? b : c ? d : e"), "(a ? b : (c ? d : e))");
    assert_eq!(expr("ok ? .5 : 1.5"), "(ok ? .5 : 1.5)");
}

#[test]
fn unary_operators() {
    assert_eq!(expr("-a.b"), "(-a.b)");
    assert_eq!(expr("!ok && x"), "((!ok) && x)");
    assert_eq!(expr("i++ + ++j"), "((i++) + (++j))");
    assert_eq!(expr("~mask"), "(~mask)");
}

#[test]
fn borrows() {
    assert_eq!(expr("&this.name"), "(&this.name)");
    assert_eq!(expr("&mut list"), "(&mut list)");
    assert_eq!(expr("&&x"), "(&(&x))");
    assert_eq!(expr("a & b"), "(a & b)");
}

#[test]
fn assignments() {
    assert_eq!(expr("a = b = c"), "(a = (b = c))");
    assert_eq!(expr("x += 1"), "(x += 1)");
    assert_eq!(expr("bits >>>= 2"), "(bits >>>= 2)");
}

#[test]
fn calls_fields_and_indexing() {
    assert_eq!(expr("Console.println(\"hi\")"), "Console.println(\"hi\")");
    assert_eq!(expr("a.b.c(1, 2)[0].d"), "a.b.c(1, 2)[0].d");
    assert_eq!(expr("foo(x)"), "foo(x)");
    assert_eq!(expr("super.toString()"), "super.toString()");
    assert_eq!(expr("person?.address?.city"), "person?.address?.city");
}

#[test]
fn method_references() {
    assert_eq!(expr("String::length"), "String::length");
    assert_eq!(expr("Person::new"), "Person::new");
    assert_eq!(expr("this::handle"), "this::handle");
}

#[test]
fn object_and_array_creation() {
    assert_eq!(expr("new Person(\"Ali\", 30)"), "new Person(\"Ali\", 30)");
    assert_eq!(expr("new ArrayList<String>()"), "new ArrayList<String>()");
    assert_eq!(expr("new ArrayList<>()"), "new ArrayList()");
    assert_eq!(expr("new int[10]"), "new int[10]");
    assert_eq!(expr("new int[] {1, 2}"), "new int[] {1, 2}");
}

#[test]
fn casts() {
    assert_eq!(expr("(int) x"), "((int) x)");
    assert_eq!(expr("(String) obj"), "((String) obj)");
    assert_eq!(expr("(List<String>) items"), "((List<String>) items)");
    assert_eq!(expr("(int) -x"), "((int) (-x))");
}

#[test]
fn parentheses_are_not_casts() {
    assert_eq!(expr("(a) + b"), "(a + b)");
    assert_eq!(expr("(a) - b"), "(a - b)");
    assert_eq!(expr("(a).b"), "a.b");
    assert_eq!(expr("(a < b)"), "(a < b)");
}

#[test]
fn instanceof() {
    assert_eq!(
        expr("x instanceof String s && s.isEmpty()"),
        "((x instanceof String s) && s.isEmpty())"
    );
    assert_eq!(
        expr("x instanceof List<String>"),
        "(x instanceof List<String>)"
    );
}

#[test]
fn lambdas() {
    assert_eq!(expr("x -> x * 2"), "((x) -> (x * 2))");
    assert_eq!(expr("(a, b) -> a + b"), "((a, b) -> (a + b))");
    assert_eq!(expr("(int a, String b) -> a"), "((int a, String b) -> a)");
    assert_eq!(expr("() -> { run(); }"), "(() -> { ... })");
    assert_eq!(expr("list.map(x -> x + 1)"), "list.map(((x) -> (x + 1)))");
    assert_eq!(expr("move x -> x"), "(move (x) -> x)");
    assert_eq!(expr("move () -> { run(); }"), "(move () -> { ... })");
    assert_eq!(expr("move(x)"), "move(x)");
    assert_eq!(expr("move -> move"), "((move) -> move)");
}

#[test]
fn literals() {
    assert_eq!(expr("'a'"), "'a'");
    assert_eq!(expr("\"text\""), "\"text\"");
    assert_eq!(expr("true"), "true");
    assert_eq!(expr("null"), "null");
    assert_eq!(expr("1.5f"), "1.5f");
    assert_eq!(expr("100L"), "100L");
}

#[test]
fn array_initializers() {
    let stmts = body("int[] xs = {1, 2, {3}};");
    let StmtKind::Local(local) = &stmts[0].kind else {
        panic!("expected a local");
    };
    assert_eq!(local.init.as_ref().unwrap().to_string(), "{1, 2, {3}}");
}

#[test]
fn switch_expressions() {
    assert_eq!(
        expr("switch (day) { case 1, 7 -> \"weekend\"; default -> \"weekday\"; }"),
        "switch (day) { case 1, 7 -> \"weekend\"; default -> \"weekday\"; }"
    );
    assert_eq!(
        expr("switch (x) { case 0 -> { yield 1; } default -> throw new Error(); }"),
        "switch (x) { case 0 -> { ... } default -> throw new Error(); }"
    );
}

#[test]
fn with_expressions() {
    assert_eq!(
        expr("user with { email = \"a\", visits = 1 }"),
        "(user with { email = \"a\", visits = 1 })"
    );
    assert_eq!(
        expr("users[0] with { visits = n + 1, }"),
        "(users[0] with { visits = (n + 1) })"
    );
}

#[test]
fn interpolated_strings_become_concatenations() {
    assert_eq!(expr(r#""Hi $name!""#), r#"(("Hi " + (&name)) + "!")"#);
    assert_eq!(expr(r#""${a + b}""#), r#"("" + (&(a + b)))"#);
    assert_eq!(
        expr(r#""[${price:>8.2}] ${ok ? 1 : 2}""#),
        r#"((("[" + (&price).format(">8.2")) + "] ") + (&(ok ? 1 : 2)))"#
    );
    assert_eq!(
        expr(r#""${"in ${x}"}\$y""#),
        r#"(("" + (&("in " + (&x)))) + "\$y")"#
    );
    assert_eq!(expr(r#""plain $ text""#), r#""plain $ text""#);
}

#[test]
fn interpolation_errors() {
    assert_eq!(
        super::errors(r#"class A { void f() { g("${x:.q}"); } }"#),
        ["invalid format spec `.q`"]
    );
    assert_eq!(
        super::errors(r#"class A { void f() { g("${x +}"); } }"#),
        ["expected expression, found `}`"]
    );
    assert_eq!(
        super::errors(r#"class A { void f() { g("${x y}"); } }"#),
        ["unexpected tokens in an interpolation"]
    );
    assert_eq!(
        super::errors(r#"class A { void f() { g("${}"); } }"#),
        ["empty interpolation"]
    );
    assert_eq!(
        super::errors(r#"class A { void f() { g("${x"); } }"#).first(),
        Some(&"this `${` is never closed".to_owned())
    );
}

#[test]
fn array_lengths_after_new_are_not_borrow_sources() {
    assert_eq!(expr("new String[count]"), "new String[count]");
    assert_eq!(expr("new List<String>[n]"), "new List<String>[n]");
}

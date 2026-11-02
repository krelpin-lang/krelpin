use krelpin_ast::ast::{CompilationUnit, ItemKind, MemberKind};

use super::{errors, parse_ok};

fn kind_name(unit: &CompilationUnit, index: usize) -> &'static str {
    match unit.items[index].kind {
        ItemKind::Class(_) => "class",
        ItemKind::Struct(_) => "struct",
        ItemKind::Interface(_) => "interface",
        ItemKind::Enum(_) => "enum",
    }
}

#[test]
fn all_item_kinds() {
    let unit = parse_ok("class A {} struct B {} interface C {} enum D { X }");
    let kinds: Vec<&str> = (0..4).map(|index| kind_name(&unit, index)).collect();
    assert_eq!(kinds, ["class", "struct", "interface", "enum"]);
}

#[test]
fn structs() {
    let unit = parse_ok(
        "struct Point implements Comparable<Point> {\n\
         \x20   int x;\n\
         \x20   int y;\n\
         }",
    );
    let ItemKind::Struct(point) = &unit.items[0].kind else {
        panic!("expected a struct");
    };
    assert_eq!(point.implements[0].to_string(), "Comparable<Point>");
    assert_eq!(point.members.len(), 2);
}

#[test]
fn interfaces() {
    let unit = parse_ok(
        "interface Shape extends Drawable, Comparable<Shape> {\n\
         \x20   const int SIDES = 0;\n\
         \x20   double area(&this);\n\
         \x20   String describe(&this) { return \"shape\"; }\n\
         }",
    );
    let ItemKind::Interface(shape) = &unit.items[0].kind else {
        panic!("expected an interface");
    };
    assert_eq!(shape.extends.len(), 2);

    let bodies: Vec<bool> = shape
        .members
        .iter()
        .filter_map(|member| match &member.kind {
            MemberKind::Method(method) => Some(method.body.is_some()),
            _ => None,
        })
        .collect();
    assert_eq!(bodies, [false, true]);
}

#[test]
fn enums() {
    let unit = parse_ok(
        "enum Shape implements Named {\n\
         \x20   Circle(double radius),\n\
         \x20   Rect(double width, double height),\n\
         \x20   Empty,\n\
         \x20   ;\n\
         \x20   const double PI = 3.14;\n\
         \x20   double area(&this) { return 0.0; }\n\
         }",
    );
    let ItemKind::Enum(shape) = &unit.items[0].kind else {
        panic!("expected an enum");
    };
    let variants: Vec<(&str, usize)> = shape
        .variants
        .iter()
        .map(|variant| (variant.name.name.as_str(), variant.fields.len()))
        .collect();
    assert_eq!(variants, [("Circle", 1), ("Rect", 2), ("Empty", 0)]);
    let fields: Vec<&str> = shape.variants[1]
        .fields
        .iter()
        .map(|field| field.name.name.as_str())
        .collect();
    assert_eq!(fields, ["width", "height"]);
    assert_eq!(shape.members.len(), 2);
}

#[test]
fn generic_enums() {
    let unit = parse_ok("enum Option<T> { Some(T value), None }");
    let ItemKind::Enum(option) = &unit.items[0].kind else {
        panic!("expected an enum");
    };
    assert_eq!(option.generics.params.len(), 1);
    assert_eq!(option.variants.len(), 2);
}

#[test]
fn simple_enums() {
    let unit = parse_ok("enum Color { RED, GREEN, BLUE }");
    let ItemKind::Enum(color) = &unit.items[0].kind else {
        panic!("expected an enum");
    };
    assert_eq!(color.variants.len(), 3);
    assert!(color.members.is_empty());
}

#[test]
fn enums_have_no_constructors() {
    assert_eq!(
        errors("enum Color { RED(\"#ff0000\") }"),
        ["enum variants declare fields, not arguments"]
    );
    assert_eq!(
        errors("enum Color { RED(); }"),
        ["the variant `RED` has no fields"]
    );
    assert_eq!(
        errors("enum Color { RED; String hex; Color(String hex) {} }"),
        [
            "enums cannot have instance fields",
            "enums cannot have constructors"
        ]
    );
}

#[test]
fn abstract_and_final() {
    let unit = parse_ok(
        "abstract class Animal {\n\
         \x20   abstract String sound(&this);\n\
         \x20   final String name(&this) { return \"animal\"; }\n\
         }\n\
         final class Dog extends Animal {\n\
         \x20   String sound(&this) { return \"woof\"; }\n\
         }",
    );
    assert!(unit.items[0].inheritance.abstract_kw.is_some());
    assert!(unit.items[1].inheritance.final_kw.is_some());
}

#[test]
fn shared_fields_keep_their_visibility() {
    let unit = parse_ok(
        "class Counter {\n\
         \x20   static AtomicInt hidden = new AtomicInt(0);\n\
         \x20   public static AtomicInt visible = new AtomicInt(0);\n\
         }",
    );
    let ItemKind::Class(counter) = &unit.items[0].kind else {
        panic!("expected a class");
    };
    assert_eq!(
        counter.members[0].vis,
        krelpin_ast::ast::Visibility::Default
    );
    assert!(matches!(
        counter.members[1].vis,
        krelpin_ast::ast::Visibility::Public(_)
    ));
}

#[test]
fn declaration_errors() {
    assert_eq!(
        errors("struct P extends Q {}"),
        ["structs cannot extend other types"]
    );
    assert_eq!(
        errors("interface I implements J {}"),
        ["interfaces cannot implement other interfaces"]
    );
    assert_eq!(
        errors("abstract struct S {}"),
        ["`abstract` cannot be used on structs"]
    );
    assert_eq!(
        errors("abstract final class C {}"),
        ["a class cannot be both `abstract` and `final`"]
    );
    assert_eq!(
        errors("final interface I {}"),
        ["`final` cannot be used on interfaces"]
    );
}

#[test]
fn member_errors() {
    assert_eq!(
        errors("interface I { int x; }"),
        ["interfaces can only have `const` fields"]
    );
    assert_eq!(
        errors("interface I { I() {} }"),
        ["interfaces cannot have constructors"]
    );
    assert_eq!(
        errors("struct Point { int x; Point(int x) { this.x = x; } }"),
        ["structs cannot have constructors"]
    );
    assert_eq!(
        errors("struct Point { int x; int twice(&this) { return this.x * 2; } }"),
        ["structs cannot have methods"]
    );
    assert_eq!(
        errors("class A { static void f(); }"),
        ["missing body for method `f`"]
    );
    assert_eq!(
        errors("class A { @Intrinsic int f(&this); }"),
        Vec::<String>::new()
    );
    assert_eq!(
        errors("abstract class A { abstract static void f() {} }"),
        ["abstract methods cannot have a body"]
    );
    assert_eq!(
        errors("abstract class A { abstract final static void f(); }"),
        ["a method cannot be both `abstract` and `final`"]
    );
    assert_eq!(
        errors("struct S { abstract static void f(); }"),
        ["structs cannot have methods"]
    );
    assert_eq!(
        errors("class A { abstract int x; }"),
        ["`abstract` cannot be used on fields"]
    );
    assert_eq!(
        errors("class A { final final static void f() {} }"),
        ["duplicate `final` modifier"]
    );
}

#[test]
fn nested_types() {
    let unit = parse_ok(
        "class List {\n\
         \x20   static class Node { int value; }\n\
         \x20   enum Kind { Single, Double }\n\
         \x20   struct Entry { int count; }\n\
         \x20   interface Visitor { void visit(&this, Node node); }\n\
         }",
    );
    let ItemKind::Class(list) = &unit.items[0].kind else {
        panic!("expected a class");
    };
    let nested: Vec<&str> = list
        .members
        .iter()
        .filter_map(|member| match &member.kind {
            MemberKind::Type(item) => Some(item.name.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(nested, ["Node", "Kind", "Entry", "Visitor"]);
    assert_eq!(
        errors("class A { class B {} }"),
        ["inner classes are not supported"]
    );
}

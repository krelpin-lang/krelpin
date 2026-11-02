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
         \x20   Point(int x, int y) { this.x = x; this.y = y; }\n\
         \x20   double length(&this) { return Math.sqrt(this.x * this.x + this.y * this.y); }\n\
         }",
    );
    let ItemKind::Struct(point) = &unit.items[0].kind else {
        panic!("expected a struct");
    };
    assert_eq!(point.implements[0].to_string(), "Comparable<Point>");
    assert_eq!(point.members.len(), 4);
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
        "enum Planet implements Named {\n\
         \x20   MERCURY(3.3e23, 2.44e6),\n\
         \x20   EARTH(5.97e24, 6.37e6),\n\
         \x20   ;\n\
         \x20   double mass;\n\
         \x20   double radius;\n\
         \x20   Planet(double mass, double radius) { this.mass = mass; this.radius = radius; }\n\
         }",
    );
    let ItemKind::Enum(planet) = &unit.items[0].kind else {
        panic!("expected an enum");
    };
    let names: Vec<&str> = planet
        .constants
        .iter()
        .map(|constant| constant.name.name.as_str())
        .collect();
    assert_eq!(names, ["MERCURY", "EARTH"]);
    assert_eq!(planet.constants[0].args.len(), 2);
    assert_eq!(planet.members.len(), 3);
}

#[test]
fn simple_enums() {
    let unit = parse_ok("enum Color { RED, GREEN, BLUE }");
    let ItemKind::Enum(color) = &unit.items[0].kind else {
        panic!("expected an enum");
    };
    assert_eq!(color.constants.len(), 3);
    assert!(color.members.is_empty());
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
         \x20   shared AtomicInt hidden = new AtomicInt(0);\n\
         \x20   public shared AtomicInt visible = new AtomicInt(0);\n\
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
        errors("enum E<T> { A }"),
        ["enums cannot have generic parameters"]
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
        errors("class A { void f(); }"),
        ["missing body for method `f`"]
    );
    assert_eq!(
        errors("class A { @Intrinsic int f(&this); }"),
        Vec::<String>::new()
    );
    assert_eq!(
        errors("abstract class A { abstract void f() {} }"),
        ["abstract methods cannot have a body"]
    );
    assert_eq!(
        errors("abstract class A { abstract final void f(); }"),
        ["a method cannot be both `abstract` and `final`"]
    );
    assert_eq!(
        errors("struct S { abstract void f(); }"),
        ["abstract methods are not allowed in structs"]
    );
    assert_eq!(
        errors("class A { abstract int x; }"),
        ["`abstract` cannot be used on fields"]
    );
    assert_eq!(
        errors("class A { final final void f() {} }"),
        ["duplicate `final` modifier"]
    );
}

use krelpin_ast::ast::{CaseLabel, PatKind, StmtKind};

use super::{body, body_errors, expr};

fn case_pats(src: &str) -> Vec<PatKind> {
    let stmts = body(&format!("switch (x) {{ {src} }}"));
    let StmtKind::Switch { arms, .. } = &stmts[0].kind else {
        panic!("expected switch");
    };
    match &arms[0].label {
        CaseLabel::Case { pats, .. } => pats.iter().map(|pat| pat.kind.clone()).collect(),
        CaseLabel::Default(_) => panic!("expected a case"),
    }
}

#[test]
fn pattern_kinds() {
    let kind = |src: &str| match case_pats(&format!("case {src} -> {{}}")).as_slice() {
        [PatKind::Wild] => "wild",
        [PatKind::Lit(_)] => "lit",
        [PatKind::Path(_)] => "path",
        [
            PatKind::Ty {
                binding: Some(_), ..
            },
        ] => "binding",
        [PatKind::Ty { binding: None, .. }] => "type",
        [PatKind::Variant { .. }] => "variant",
        other => panic!("unexpected patterns {other:?}"),
    };
    assert_eq!(kind("_"), "wild");
    assert_eq!(kind("-1"), "lit");
    assert_eq!(kind("\"a\""), "lit");
    assert_eq!(kind("RED"), "path");
    assert_eq!(kind("Limits.MAX"), "path");
    assert_eq!(kind("String s"), "binding");
    assert_eq!(kind("String _"), "type");
    assert_eq!(kind("List<String> items"), "binding");
    assert_eq!(kind("int[] values"), "binding");
    assert_eq!(kind("Circle(double r)"), "variant");
    assert_eq!(kind("Shape.Rect(double w, _)"), "variant");
}

#[test]
fn nested_patterns_and_guards() {
    assert_eq!(
        expr(
            "switch (tree) { case Node(Leaf(int v), _) when v > 0 -> v; \
             case Leaf(_), Empty -> 0; }"
        ),
        "switch (tree) { case Node(Leaf(int v), _) when (v > 0) -> v; \
         case Leaf(_), Empty -> 0; }"
    );
}

#[test]
fn instanceof_takes_a_pattern() {
    assert_eq!(
        expr("shape instanceof Circle(double r) && r > 1"),
        "((shape instanceof Circle(double r)) && (r > 1))"
    );
    assert_eq!(
        body_errors("boolean b = x instanceof 1;"),
        ["`instanceof` needs a type or a variant"]
    );
}

#[test]
fn guards_are_not_lambdas() {
    assert_eq!(
        expr("switch (x) { case Leaf(boolean ready) when ready -> 1; default -> 0; }"),
        "switch (x) { case Leaf(boolean ready) when ready -> 1; default -> 0; }"
    );
}

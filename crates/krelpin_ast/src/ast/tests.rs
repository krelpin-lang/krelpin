use krelpin_span::{Span, Symbol};

use super::{Ident, Path, PrimitiveTy, Ty, TyKind};

fn ty(kind: TyKind) -> Ty {
    Ty {
        kind,
        span: Span::DUMMY,
    }
}

fn ident(name: &str) -> Ident {
    Ident::new(Symbol::intern(name), Span::DUMMY)
}

fn path(name: &str, args: Vec<Ty>) -> Ty {
    ty(TyKind::Path {
        path: Path::new(name.split('.').map(ident).collect()),
        args,
    })
}

fn reference(mutable: bool, sources: &[&str], inner: Ty) -> Ty {
    ty(TyKind::Ref {
        mutable,
        sources: sources.iter().copied().map(ident).collect(),
        ty: Box::new(inner),
    })
}

fn nullable(inner: Ty) -> Ty {
    ty(TyKind::Nullable(Box::new(inner)))
}

#[test]
fn displays_paths_and_generics() {
    let string = path("String", vec![]);
    let map = path(
        "java.util.Map",
        vec![string.clone(), path("List", vec![string])],
    );
    assert_eq!(map.to_string(), "java.util.Map<String, List<String>>");
}

#[test]
fn displays_references() {
    let string = path("String", vec![]);
    assert_eq!(reference(false, &[], string.clone()).to_string(), "&String");
    assert_eq!(
        reference(true, &["a", "b"], string).to_string(),
        "&mut [a, b] String"
    );
}

#[test]
fn displays_nullable_references_unambiguously() {
    let string = path("String", vec![]);
    assert_eq!(
        nullable(reference(false, &[], string.clone())).to_string(),
        "&String?"
    );
    assert_eq!(
        reference(false, &[], nullable(string)).to_string(),
        "&(String?)"
    );
}

#[test]
fn primitives_round_trip() {
    for primitive in [
        PrimitiveTy::Boolean,
        PrimitiveTy::Byte,
        PrimitiveTy::Short,
        PrimitiveTy::Int,
        PrimitiveTy::Long,
        PrimitiveTy::Char,
        PrimitiveTy::Float,
        PrimitiveTy::Double,
    ] {
        let symbol = Symbol::intern(primitive.name());
        assert_eq!(PrimitiveTy::from_symbol(symbol), Some(primitive));
    }
    assert_eq!(PrimitiveTy::from_symbol(Symbol::intern("String")), None);
}

#[test]
fn displays_arrays() {
    let string = path("String", vec![]);
    let array = |inner| ty(TyKind::Array(Box::new(inner)));
    assert_eq!(array(string.clone()).to_string(), "String[]");
    assert_eq!(
        reference(false, &[], array(string.clone())).to_string(),
        "&String[]"
    );
    assert_eq!(
        array(reference(false, &[], string.clone())).to_string(),
        "(&String)[]"
    );
    assert_eq!(array(nullable(string)).to_string(), "(String?)[]");
}

use crate::body::is_concrete;
use crate::func::{float_const, float32_const};
use crate::layout::{field_offsets, struct_layout};
use krelpin_ast::ast::PrimitiveTy;
use krelpin_typeck::Ty;

#[test]
fn structs_follow_the_c_layout() {
    assert_eq!(struct_layout(&[(4, 4), (8, 8)]), (16, 8));
    assert_eq!(struct_layout(&[(1, 1), (4, 4), (1, 1)]), (12, 4));
    assert_eq!(struct_layout(&[]), (0, 1));
    assert_eq!(
        field_offsets(&[(4, 4), (4, 4), (8, 8), (1, 1)]),
        [0, 4, 8, 16]
    );
}

#[test]
fn floats_are_written_as_exact_bits() {
    assert_eq!(float_const(1.0), "0x3FF0000000000000");
    assert_eq!(
        float32_const(0.1),
        format!("0x{:016X}", f64::from(0.1f32).to_bits())
    );
}

#[test]
fn only_concrete_types_reach_the_backend() {
    let int = Ty::Primitive(PrimitiveTy::Int);
    assert!(is_concrete(&Ty::Array(Box::new(int.clone()))));
    assert!(!is_concrete(&Ty::Nullable(Box::new(Ty::Param(
        krelpin_span::Symbol::intern("T")
    )))));
    assert!(!is_concrete(&Ty::Ref {
        mutable: false,
        ty: Box::new(Ty::Unknown),
    }));
}

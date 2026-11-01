mod check;
mod coerce;
mod expr;
mod sig;
mod stmt;
pub mod ty;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use krelpin_ast::ast::CompilationUnit;
use krelpin_errors::DiagCtxt;
use krelpin_resolve::Resolutions;
use krelpin_span::Span;

pub use crate::sig::{AdtKind, FieldSig, MethodSig, Tcx, TypeSig};
pub use crate::ty::Ty;

#[derive(Debug, Default)]
pub struct TypeckResults {
    pub expr_types: HashMap<Span, Ty>,
}

pub fn check(unit: &CompilationUnit, resolutions: &Resolutions, dcx: &DiagCtxt) -> TypeckResults {
    let mut tcx = Tcx::new(unit, resolutions, dcx);
    tcx.break_inheritance_cycles();
    check::check_unit(&tcx, unit)
}

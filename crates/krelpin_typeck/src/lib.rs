mod call;
mod check;
mod closure;
mod coerce;
mod exceptions;
mod expr;
mod inherit;
mod lang_items;
mod param_env;
mod pat;
mod sig;
mod stmt;
pub mod ty;
mod usefulness;
mod wfcheck;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use krelpin_ast::ast::CompilationUnit;
use krelpin_errors::DiagCtxt;
use krelpin_resolve::ResolverOutput;
use krelpin_span::Span;

pub use crate::closure::FnSig;
pub use crate::lang_items::{LangItem, LangItems};
pub use crate::param_env::ParamEnv;
pub use crate::sig::{
    AdtKind, FieldSig, GenericParamSig, MethodSig, Tcx, TypeSig, VariantFieldSig, VariantSig,
};
pub use crate::ty::Ty;

#[derive(Debug, Default)]
pub struct TypeckResults {
    pub expr_types: HashMap<Span, Ty>,
    pub callees: HashMap<Span, Callee>,
    pub fields: HashMap<Span, FieldSig>,
}

impl TypeckResults {
    fn extend(&mut self, other: TypeckResults) {
        self.expr_types.extend(other.expr_types);
        self.callees.extend(other.callees);
        self.fields.extend(other.fields);
    }
}

#[derive(Clone, Debug)]
pub struct Callee {
    pub method: MethodSig,
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub packs_varargs: bool,
}

pub fn check(
    units: &[CompilationUnit],
    resolved: &ResolverOutput,
    dcx: &DiagCtxt,
) -> TypeckResults {
    let mut tcx = Tcx::new(units, resolved, dcx);
    tcx.break_inheritance_cycles();
    check::check_units(&tcx, units)
}

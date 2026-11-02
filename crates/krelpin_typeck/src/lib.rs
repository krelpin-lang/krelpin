mod call;
mod check;
mod closure;
mod coerce;
mod expr;
mod lang_items;
mod param_env;
mod sig;
mod stmt;
pub mod ty;
mod wfcheck;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use krelpin_ast::ast::CompilationUnit;
use krelpin_errors::DiagCtxt;
use krelpin_resolve::ResolverOutput;
use krelpin_span::Span;

pub use crate::closure::FnSig;
pub use crate::coerce::Coercion;
pub use crate::lang_items::{LangItem, LangItems};
pub use crate::param_env::ParamEnv;
pub use crate::sig::{AdtKind, FieldSig, GenericParamSig, MethodSig, Tcx, TypeSig};
pub use crate::ty::Ty;

#[derive(Debug, Default)]
pub struct TypeckResults {
    pub expr_types: HashMap<Span, Ty>,
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

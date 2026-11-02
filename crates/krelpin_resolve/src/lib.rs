mod collect;
mod late;
mod package;
mod prelude;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use krelpin_ast::ast::{CompilationUnit, PrimitiveTy};
use krelpin_errors::DiagCtxt;
use krelpin_span::Span;
use krelpin_span::def_id::DefId;

pub use crate::collect::{FieldDef, ItemTable, MethodDef, TypeDef, UnitScope};
pub use crate::package::PackagePath;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Res {
    Local(Span),
    TypeParam(Span),
    BorrowSource(Span),
    Item(DefId),
    PrimTy(PrimitiveTy),
    Import(Span),
}

#[derive(Debug, Default)]
pub struct Resolutions {
    names: HashMap<Span, Res>,
}

impl Resolutions {
    pub fn get(&self, span: Span) -> Option<Res> {
        self.names.get(&span).copied()
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    fn record(&mut self, span: Span, res: Res) {
        self.names.insert(span, res);
    }
}

pub struct ResolverOutput {
    pub items: ItemTable,
    pub resolutions: Resolutions,
}

pub fn resolve(units: &[CompilationUnit], dcx: &DiagCtxt) -> ResolverOutput {
    let items = collect::collect(units, dcx);
    let resolutions = late::resolve_bodies(units, &items, dcx);
    ResolverOutput { items, resolutions }
}

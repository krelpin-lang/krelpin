mod collect;
mod late;
mod prelude;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use krelpin_ast::ast::CompilationUnit;
use krelpin_errors::DiagCtxt;
use krelpin_span::{Span, Symbol};

pub use crate::collect::{FieldDef, ItemTable, MethodDef, TypeDef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Res {
    Local(Span),
    TypeParam(Span),
    BorrowSource(Span),
    Item(usize),
    Import(Span),
    Prelude(Symbol),
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

pub fn resolve(unit: &CompilationUnit, dcx: &DiagCtxt) -> ResolverOutput {
    let items = collect::collect(unit, dcx);
    let resolutions = late::resolve_bodies(unit, &items, dcx);
    ResolverOutput { items, resolutions }
}

use std::collections::HashMap;

use krelpin_span::Symbol;

use crate::sig::GenericParamSig;
use crate::ty::Ty;

#[derive(Clone, Debug, Default)]
pub struct ParamEnv {
    bounds: HashMap<Symbol, Vec<Ty>>,
}

impl ParamEnv {
    pub fn with_generics(mut self, generics: &[GenericParamSig]) -> ParamEnv {
        for param in generics {
            self.bounds.insert(param.name, param.bounds.clone());
        }
        self
    }

    pub fn with_where_bounds(mut self, predicates: &[GenericParamSig]) -> ParamEnv {
        for predicate in predicates {
            self.bounds
                .entry(predicate.name)
                .or_default()
                .extend(predicate.bounds.iter().cloned());
        }
        self
    }

    pub fn bounds(&self, name: Symbol) -> &[Ty] {
        self.bounds.get(&name).map_or(&[], Vec::as_slice)
    }
}

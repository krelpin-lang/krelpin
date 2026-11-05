use std::collections::HashMap;

use krelpin_span::Symbol;

use crate::sig::GenericParamSig;
use crate::ty::Ty;

#[derive(Clone, Debug, Default)]
pub struct ParamEnv {
    bounds: HashMap<Symbol, Vec<Ty>>,
    /// Parameters that had a bound removed because it made them extend
    /// themselves, like `T extends U, U extends T`.
    cyclic: Vec<Symbol>,
}

impl ParamEnv {
    pub fn with_generics(mut self, generics: &[GenericParamSig]) -> ParamEnv {
        for param in generics {
            self.bounds.insert(param.name, param.bounds.clone());
        }
        self.break_cycles();
        self
    }

    pub fn with_where_bounds(mut self, predicates: &[GenericParamSig]) -> ParamEnv {
        for predicate in predicates {
            self.bounds
                .entry(predicate.name)
                .or_default()
                .extend(predicate.bounds.iter().cloned());
        }
        self.break_cycles();
        self
    }

    pub fn bounds(&self, name: Symbol) -> &[Ty] {
        self.bounds.get(&name).map_or(&[], Vec::as_slice)
    }

    pub fn is_cyclic(&self, name: Symbol) -> bool {
        self.cyclic.contains(&name)
    }

    /// Drops every bound that leads back to its own parameter, so that code
    /// walking from a parameter to its bounds always ends.
    fn break_cycles(&mut self) {
        let mut names: Vec<Symbol> = self.bounds.keys().copied().collect();
        names.sort_by_key(|name| name.as_str().to_owned());
        for name in names {
            let bounds = self.bounds[&name].clone();
            let kept: Vec<Ty> = bounds
                .into_iter()
                .filter(|bound| !self.reaches(bound, name))
                .collect();
            if kept.len() != self.bounds[&name].len() {
                self.cyclic.push(name);
                self.bounds.insert(name, kept);
            }
        }
    }

    /// Whether following parameter bounds from `ty` arrives at `target`.
    fn reaches(&self, ty: &Ty, target: Symbol) -> bool {
        let mut stack = vec![ty];
        let mut seen = Vec::new();
        while let Some(ty) = stack.pop() {
            let Ty::Param(name) = ty else {
                continue;
            };
            if *name == target {
                return true;
            }
            if seen.contains(name) {
                continue;
            }
            seen.push(*name);
            stack.extend(self.bounds(*name));
        }
        false
    }
}

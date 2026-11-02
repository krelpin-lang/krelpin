use std::fmt;

use krelpin_ast::ast::{CompilationUnit, Ident};
use krelpin_span::Symbol;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PackagePath(Vec<Symbol>);

impl PackagePath {
    pub fn parse(dotted: &str) -> PackagePath {
        PackagePath(dotted.split('.').map(Symbol::intern).collect())
    }

    pub fn is_std(&self) -> bool {
        self.0.first().is_some_and(|name| name.as_str() == "std")
    }

    pub fn from_idents(idents: &[Ident]) -> PackagePath {
        PackagePath(idents.iter().map(|ident| ident.name).collect())
    }

    pub fn of_unit(unit: &CompilationUnit) -> PackagePath {
        unit.package
            .as_ref()
            .map_or_else(PackagePath::default, |path| {
                PackagePath::from_idents(&path.segments)
            })
    }
}

impl fmt::Display for PackagePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.0.iter().map(|name| name.as_str()).collect();
        f.write_str(&names.join("."))
    }
}

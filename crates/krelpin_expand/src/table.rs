use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use krelpin_ast::ast::{CompilationUnit, Import, MacroDef, Path};
use krelpin_ast::package::PackagePath;
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_span::{Span, Symbol};

use crate::proc_macro::ProcMacroCrate;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MacroKind {
    Extern {
        krate: usize,
        index: usize,
        name: Symbol,
    },
    Local {
        name: Symbol,
    },
}

impl MacroKind {
    pub(crate) fn name(self) -> Symbol {
        match self {
            MacroKind::Extern { name, .. } | MacroKind::Local { name } => name,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct MacroTable {
    macros: HashMap<PackagePath, HashMap<Symbol, MacroKind>>,
    types: HashSet<(PackagePath, Symbol)>,
    packages: HashSet<PackagePath>,
}

impl MacroTable {
    pub(crate) fn collect(
        units: &[CompilationUnit],
        externs: &[ProcMacroCrate],
        proc_macro: bool,
        dcx: &DiagCtxt,
    ) -> MacroTable {
        let mut table = MacroTable::default();
        for unit in units {
            let package = PackagePath::of_unit(unit);
            table.packages.insert(package.clone());
            for item in &unit.items {
                table.types.insert((package.clone(), item.name.name));
            }
            for def in &unit.macros {
                table.add(&package, def, proc_macro, dcx);
            }
        }
        for (krate, extern_crate) in externs.iter().enumerate() {
            for (index, path) in extern_crate.macros().iter().enumerate() {
                let (package, name) = match path.rsplit_once('.') {
                    Some((package, name)) => (PackagePath::parse(package), name),
                    None => (PackagePath::default(), path.as_str()),
                };
                let name = Symbol::intern(name);
                let kind = MacroKind::Extern { krate, index, name };
                if table.insert(&package, name, kind).is_err() {
                    dcx.emit(
                        Diagnostic::error(format!(
                            "the macro `{path}` of `{}` is already defined",
                            extern_crate.name()
                        ))
                        .with_note("two crates cannot define a macro with the same path"),
                    );
                }
            }
        }
        table
    }

    fn add(&mut self, package: &PackagePath, def: &MacroDef, proc_macro: bool, dcx: &DiagCtxt) {
        let name = def.name;
        let kind = if package.is_std() {
            dcx.emit(
                Diagnostic::error(format!("unknown intrinsic macro `{package}.{}`", name.name))
                    .with_span(name.span)
                    .with_note("the standard library defines no macros"),
            );
            return;
        } else if !proc_macro || def.body.is_none() {
            return;
        } else {
            MacroKind::Local { name: name.name }
        };
        if self.insert(package, name.name, kind).is_err() {
            dcx.emit(
                Diagnostic::error(format!(
                    "the macro `{}` is defined multiple times",
                    name.name
                ))
                .with_span(name.span),
            );
        }
    }

    fn insert(&mut self, package: &PackagePath, name: Symbol, kind: MacroKind) -> Result<(), ()> {
        match self.macros.entry(package.clone()).or_default().entry(name) {
            Entry::Occupied(_) => Err(()),
            Entry::Vacant(entry) => {
                entry.insert(kind);
                Ok(())
            }
        }
    }

    pub(crate) fn lookup(&self, package: &PackagePath, name: Symbol) -> Option<MacroKind> {
        self.macros.get(package)?.get(&name).copied()
    }

    fn has_type(&self, package: &PackagePath, name: Symbol) -> bool {
        self.types.contains(&(package.clone(), name))
    }

    fn only_macros(&self, package: &PackagePath) -> bool {
        self.macros.contains_key(package) && !self.packages.contains(package)
    }
}

pub(crate) struct MacroScope<'a> {
    table: &'a MacroTable,
    package: PackagePath,
    imports: HashMap<Symbol, MacroKind>,
    globs: Vec<PackagePath>,
    prelude: Vec<PackagePath>,
    macro_only_imports: Vec<Span>,
}

impl<'a> MacroScope<'a> {
    pub(crate) fn new(table: &'a MacroTable, unit: &CompilationUnit) -> MacroScope<'a> {
        let mut scope = MacroScope {
            table,
            package: PackagePath::of_unit(unit),
            imports: HashMap::new(),
            globs: Vec::new(),
            prelude: PackagePath::prelude(),
            macro_only_imports: Vec::new(),
        };
        for import in &unit.imports {
            scope.add_import(import);
        }
        scope
    }

    fn add_import(&mut self, import: &Import) {
        if import.glob {
            let package = PackagePath::from_idents(&import.path.segments);
            if self.table.only_macros(&package) {
                self.macro_only_imports.push(import.span);
            }
            self.globs.push(package);
            return;
        }
        let Some((name, package)) = import.path.segments.split_last() else {
            return;
        };
        let package = PackagePath::from_idents(package);
        let Some(kind) = self.table.lookup(&package, name.name) else {
            return;
        };
        self.imports.insert(name.name, kind);
        if !self.table.has_type(&package, name.name) {
            self.macro_only_imports.push(import.span);
        }
    }

    pub(crate) fn is_macro_import(&self, import: &Import) -> bool {
        self.macro_only_imports.contains(&import.span)
    }

    pub(crate) fn resolve(&self, path: &Path) -> Option<MacroKind> {
        let (name, package) = path.segments.split_last()?;
        if !package.is_empty() {
            return self
                .table
                .lookup(&PackagePath::from_idents(package), name.name);
        }
        if let Some(&kind) = self.imports.get(&name.name) {
            return Some(kind);
        }
        std::iter::once(&self.package)
            .chain(&self.globs)
            .chain(&self.prelude)
            .find_map(|package| self.table.lookup(package, name.name))
    }
}

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use krelpin_ast::ast::{CompilationUnit, Import, MacroDef, Path, is_intrinsic};
use krelpin_ast::package::PackagePath;
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_span::{Span, Symbol};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinMacro {
    Derive,
}

impl BuiltinMacro {
    const ALL: &[BuiltinMacro] = &[BuiltinMacro::Derive];

    pub fn name(self) -> &'static str {
        match self {
            BuiltinMacro::Derive => "Derive",
        }
    }

    pub fn path(self) -> String {
        format!("std.lang.{}", self.name())
    }

    pub fn usage(self) -> &'static str {
        match self {
            BuiltinMacro::Derive => "`@Derive` can only be used on classes, structs and enums",
        }
    }

    fn lookup(package: &PackagePath, name: Symbol) -> Option<BuiltinMacro> {
        let path = format!("{package}.{name}");
        Self::ALL
            .iter()
            .copied()
            .find(|builtin| builtin.path() == path)
    }
}

#[derive(Debug, Default)]
pub(crate) struct MacroTable {
    macros: HashMap<PackagePath, HashMap<Symbol, BuiltinMacro>>,
    types: HashSet<(PackagePath, Symbol)>,
}

impl MacroTable {
    pub(crate) fn collect(units: &[CompilationUnit], dcx: &DiagCtxt) -> MacroTable {
        let mut table = MacroTable::default();
        for unit in units {
            let package = PackagePath::of_unit(unit);
            for item in &unit.items {
                table.types.insert((package.clone(), item.name.name));
            }
            for def in &unit.macros {
                table.add(&package, def, dcx);
            }
        }
        table
    }

    fn add(&mut self, package: &PackagePath, def: &MacroDef, dcx: &DiagCtxt) {
        let name = def.name;
        if !package.is_std() {
            dcx.emit(
                Diagnostic::error("only the standard library can declare macros for now")
                    .with_span(name.span)
                    .with_note("macros written in Krelpin are not supported yet"),
            );
            return;
        }
        let builtin = match BuiltinMacro::lookup(package, name.name) {
            Some(builtin) if is_intrinsic(&def.annotations) => builtin,
            _ => {
                dcx.emit(
                    Diagnostic::error(format!("unknown intrinsic macro `{package}.{}`", name.name))
                        .with_span(name.span)
                        .with_note("the compiler provides the macros of the standard library"),
                );
                return;
            }
        };
        match self
            .macros
            .entry(package.clone())
            .or_default()
            .entry(name.name)
        {
            Entry::Occupied(_) => dcx.emit(
                Diagnostic::error(format!(
                    "the macro `{}` is defined multiple times",
                    name.name
                ))
                .with_span(name.span),
            ),
            Entry::Vacant(entry) => {
                entry.insert(builtin);
            }
        }
    }

    pub(crate) fn lookup(&self, package: &PackagePath, name: Symbol) -> Option<BuiltinMacro> {
        self.macros.get(package)?.get(&name).copied()
    }

    fn has_type(&self, package: &PackagePath, name: Symbol) -> bool {
        self.types.contains(&(package.clone(), name))
    }
}

pub(crate) struct MacroScope<'a> {
    table: &'a MacroTable,
    package: PackagePath,
    imports: HashMap<Symbol, BuiltinMacro>,
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
            self.globs
                .push(PackagePath::from_idents(&import.path.segments));
            return;
        }
        let Some((name, package)) = import.path.segments.split_last() else {
            return;
        };
        let package = PackagePath::from_idents(package);
        let Some(builtin) = self.table.lookup(&package, name.name) else {
            return;
        };
        self.imports.insert(name.name, builtin);
        if !self.table.has_type(&package, name.name) {
            self.macro_only_imports.push(import.span);
        }
    }

    pub(crate) fn is_macro_import(&self, import: &Import) -> bool {
        self.macro_only_imports.contains(&import.span)
    }

    pub(crate) fn resolve(&self, path: &Path) -> Option<BuiltinMacro> {
        let (name, package) = path.segments.split_last()?;
        if !package.is_empty() {
            return self
                .table
                .lookup(&PackagePath::from_idents(package), name.name);
        }
        if let Some(&builtin) = self.imports.get(&name.name) {
            return Some(builtin);
        }
        std::iter::once(&self.package)
            .chain(&self.globs)
            .chain(&self.prelude)
            .find_map(|package| self.table.lookup(package, name.name))
    }
}

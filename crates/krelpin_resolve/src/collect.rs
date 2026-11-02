use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use krelpin_ast::ast::{
    CompilationUnit, FieldStorage, Ident, Import, Item, ItemKind, Member, MemberKind, Method,
    Variant, is_intrinsic,
};
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_span::def_id::DefId;
use krelpin_span::{Span, Symbol};

use crate::{PackagePath, Res};

#[derive(Debug, Default)]
pub struct ItemTable {
    pub defs: Vec<TypeDef>,
    pub packages: HashMap<PackagePath, HashMap<Symbol, DefId>>,
    pub scopes: Vec<UnitScope>,
}

#[derive(Debug)]
pub struct TypeDef {
    pub def: DefId,
    pub name: Ident,
    pub package: PackagePath,
    pub unit: usize,
    pub item: usize,
    pub fields: HashMap<Symbol, FieldDef>,
    pub methods: HashMap<Symbol, Vec<MethodDef>>,
}

#[derive(Clone, Copy, Debug)]
pub struct FieldDef {
    pub span: Span,
    pub is_instance: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct MethodDef {
    pub span: Span,
    pub has_receiver: bool,
}

#[derive(Debug, Default)]
pub struct UnitScope {
    pub package: PackagePath,
    pub imports: HashMap<Symbol, Res>,
    pub globs: Vec<PackagePath>,
    pub has_unknown_glob: bool,
}

impl ItemTable {
    pub fn def(&self, def: DefId) -> &TypeDef {
        &self.defs[def.index()]
    }

    pub fn lookup(&self, package: &PackagePath, name: Symbol) -> Option<DefId> {
        self.packages.get(package)?.get(&name).copied()
    }

    pub fn lookup_path(&self, path: &str) -> Option<DefId> {
        let (package, name) = path.rsplit_once('.')?;
        self.lookup(&PackagePath::parse(package), Symbol::intern(name))
    }

    pub fn items<'u>(
        &self,
        units: &'u [CompilationUnit],
    ) -> impl Iterator<Item = (&TypeDef, &'u Item)> {
        self.defs
            .iter()
            .map(move |def| (def, &units[def.unit].items[def.item]))
    }
}

pub(crate) fn collect(units: &[CompilationUnit], dcx: &DiagCtxt) -> ItemTable {
    let mut table = ItemTable::default();

    for (unit_index, unit) in units.iter().enumerate() {
        let package = PackagePath::of_unit(unit);

        for (item_index, item) in unit.items.iter().enumerate() {
            let def = DefId::from_index(table.defs.len());
            let names = table.packages.entry(package.clone()).or_default();
            match names.entry(item.name.name) {
                Entry::Occupied(_) => report_redefinition(dcx, "name", item.name),
                Entry::Vacant(entry) => {
                    entry.insert(def);
                }
            }
            let type_def = collect_type(dcx, def, &package, unit_index, item_index, item);
            table.defs.push(type_def);
        }
    }

    table.scopes = units
        .iter()
        .map(|unit| collect_imports(&table, unit, dcx))
        .collect();
    table
}

fn collect_type(
    dcx: &DiagCtxt,
    def: DefId,
    package: &PackagePath,
    unit: usize,
    item_index: usize,
    item: &Item,
) -> TypeDef {
    let mut type_def = TypeDef {
        def,
        name: item.name,
        package: package.clone(),
        unit,
        item: item_index,
        fields: HashMap::new(),
        methods: HashMap::new(),
    };

    let members: &[Member] = match &item.kind {
        ItemKind::Class(class) => &class.members,
        ItemKind::Struct(def) => &def.members,
        ItemKind::Interface(interface) => &interface.members,
        ItemKind::Enum(enum_def) => &enum_def.members,
    };

    for member in members {
        match &member.kind {
            MemberKind::Field(field) => {
                let is_instance = field.storage == FieldStorage::Instance;
                add_field(dcx, &mut type_def, field.name, is_instance);
            }
            MemberKind::Method(method) => {
                if is_intrinsic(&member.annotations) && !package.is_std() {
                    dcx.emit(
                        Diagnostic::error("only the standard library can declare intrinsics")
                            .with_span(method.name.span)
                            .with_help("give the method a body, or mark it `abstract`"),
                    );
                }
                add_method(&mut type_def, method);
            }
            MemberKind::Constructor(_) => {}
        }
    }

    if let ItemKind::Enum(enum_def) = &item.kind {
        add_variants(dcx, &mut type_def, &enum_def.variants);
    }
    type_def
}

fn add_variants(dcx: &DiagCtxt, def: &mut TypeDef, variants: &[Variant]) {
    let mut seen = HashSet::new();
    for variant in variants {
        let name = variant.name;
        if !seen.insert(name.name) {
            report_redefinition(dcx, "variant", name);
            continue;
        }
        if def.fields.contains_key(&name.name) || def.methods.contains_key(&name.name) {
            dcx.emit(
                Diagnostic::error(format!(
                    "the variant `{}` has the same name as a member of the enum",
                    name.name
                ))
                .with_span(name.span)
                .with_help("rename the variant or the member"),
            );
            continue;
        }
        if variant.fields.is_empty() {
            add_field(dcx, def, name, false);
        } else {
            def.methods.entry(name.name).or_default().push(MethodDef {
                span: name.span,
                has_receiver: false,
            });
        }
    }
}

fn collect_imports(table: &ItemTable, unit: &CompilationUnit, dcx: &DiagCtxt) -> UnitScope {
    let mut scope = UnitScope {
        package: PackagePath::of_unit(unit),
        ..UnitScope::default()
    };
    let defined_here: HashSet<Symbol> = unit.items.iter().map(|item| item.name.name).collect();

    for import in &unit.imports {
        if import.glob {
            let package = PackagePath::from_idents(&import.path.segments);
            if table.packages.contains_key(&package) {
                scope.globs.push(package);
            } else {
                scope.has_unknown_glob = true;
            }
            continue;
        }

        let Some(&name) = import.path.segments.last() else {
            continue;
        };
        if defined_here.contains(&name.name) || scope.imports.contains_key(&name.name) {
            report_redefinition(dcx, "name", name);
            continue;
        }
        if let Some(res) = resolve_import(table, import, dcx) {
            scope.imports.insert(name.name, res);
        }
    }

    scope
}

fn resolve_import(table: &ItemTable, import: &Import, dcx: &DiagCtxt) -> Option<Res> {
    let (name, package) = import.path.segments.split_last()?;
    let package = PackagePath::from_idents(package);

    let Some(types) = table.packages.get(&package) else {
        return Some(Res::Import(name.span));
    };
    match types.get(&name.name) {
        Some(&def) => Some(Res::Item(def)),
        None => {
            dcx.emit(
                Diagnostic::error(format!(
                    "cannot find type `{}` in package `{package}`",
                    name.name
                ))
                .with_span(name.span)
                .with_label("not found in this package"),
            );
            None
        }
    }
}

fn add_field(dcx: &DiagCtxt, def: &mut TypeDef, name: Ident, is_instance: bool) {
    if def.fields.contains_key(&name.name) {
        report_redefinition(dcx, "field", name);
        return;
    }
    def.fields.insert(
        name.name,
        FieldDef {
            span: name.span,
            is_instance,
        },
    );
}

fn add_method(def: &mut TypeDef, method: &Method) {
    def.methods
        .entry(method.name.name)
        .or_default()
        .push(MethodDef {
            span: method.name.span,
            has_receiver: method.receiver.is_some(),
        });
}

fn report_redefinition(dcx: &DiagCtxt, what: &str, name: Ident) {
    dcx.emit(
        Diagnostic::error(format!(
            "the {what} `{}` is defined multiple times",
            name.name
        ))
        .with_span(name.span)
        .with_label(format!("`{}` redefined here", name.name))
        .with_note(format!("`{}` must be defined only once", name.name)),
    );
}

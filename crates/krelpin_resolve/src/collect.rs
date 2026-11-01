use std::collections::{HashMap, HashSet};

use krelpin_ast::ast::{
    CompilationUnit, FieldStorage, Ident, ItemKind, Member, MemberKind, Method,
};
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_span::{Span, Symbol};

#[derive(Debug, Default)]
pub struct ItemTable {
    pub types: HashMap<Symbol, TypeDef>,
    pub imports: HashMap<Symbol, Span>,
    pub has_glob_import: bool,
}

#[derive(Debug)]
pub struct TypeDef {
    pub index: usize,
    pub name: Ident,
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

pub(crate) fn collect(unit: &CompilationUnit, dcx: &DiagCtxt) -> ItemTable {
    let mut table = ItemTable::default();
    let mut defined: HashSet<Symbol> = HashSet::new();

    for import in &unit.imports {
        if import.glob {
            table.has_glob_import = true;
            continue;
        }
        let Some(last) = import.path.segments.last() else {
            continue;
        };
        if !defined.insert(last.name) {
            report_redefinition(dcx, "name", *last);
            continue;
        }
        table.imports.insert(last.name, last.span);
    }

    for (index, item) in unit.items.iter().enumerate() {
        if !defined.insert(item.name.name) {
            report_redefinition(dcx, "name", item.name);
            continue;
        }

        let mut def = TypeDef {
            index,
            name: item.name,
            fields: HashMap::new(),
            methods: HashMap::new(),
        };

        let members: &[Member] = match &item.kind {
            ItemKind::Class(class) => &class.members,
            ItemKind::Struct(def) => &def.members,
            ItemKind::Interface(interface) => &interface.members,
            ItemKind::Enum(enum_def) => {
                for constant in &enum_def.constants {
                    add_field(dcx, &mut def, constant.name, false);
                }
                &enum_def.members
            }
        };

        for member in members {
            match &member.kind {
                MemberKind::Field(field) => {
                    let is_instance = field.storage == FieldStorage::Instance;
                    add_field(dcx, &mut def, field.name, is_instance);
                }
                MemberKind::Method(method) => add_method(&mut def, method),
                MemberKind::Constructor(_) => {}
            }
        }

        table.types.insert(item.name.name, def);
    }

    table
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

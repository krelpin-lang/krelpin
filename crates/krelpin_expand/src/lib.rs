mod derive;
mod item;
mod table;

#[cfg(test)]
mod tests;

use krelpin_ast::ast::{Annotation, CompilationUnit, Item, ItemKind, Member, MemberKind};
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_span::SourceMap;

pub use crate::table::BuiltinMacro;
use crate::table::{MacroScope, MacroTable};

const BUILTIN_ATTRIBUTES: &[&str] = &["Intrinsic"];

pub fn expand(units: &mut [CompilationUnit], source_map: &SourceMap, dcx: &DiagCtxt) {
    let table = MacroTable::collect(units, dcx);
    for unit in units.iter_mut() {
        let scope = MacroScope::new(&table, unit);
        unit.imports.retain(|import| !scope.is_macro_import(import));
        let cx = ExpandCtxt {
            scope,
            source_map,
            dcx,
        };
        for item in &mut unit.items {
            cx.expand_item(item);
        }
        for def in &mut unit.macros {
            cx.reject_macros(&mut def.annotations, "macros");
        }
    }
}

pub(crate) struct Invocation {
    builtin: BuiltinMacro,
    annotation: Annotation,
}

pub(crate) struct ExpandCtxt<'a> {
    scope: MacroScope<'a>,
    source_map: &'a SourceMap,
    dcx: &'a DiagCtxt,
}

impl ExpandCtxt<'_> {
    fn expand_item(&self, item: &mut Item) {
        for invocation in self.take_invocations(&mut item.annotations) {
            match invocation.builtin {
                BuiltinMacro::Derive => self.expand_derive(item, &invocation),
            }
        }

        let members: &mut Vec<Member> = match &mut item.kind {
            ItemKind::Class(class) => &mut class.members,
            ItemKind::Struct(def) => &mut def.members,
            ItemKind::Interface(interface) => &mut interface.members,
            ItemKind::Enum(def) => {
                for variant in &mut def.variants {
                    self.reject_macros(&mut variant.annotations, "enum variants");
                }
                &mut def.members
            }
        };
        for member in members {
            self.reject_macros(&mut member.annotations, "members");
            if let MemberKind::Type(nested) = &mut member.kind {
                self.expand_item(nested);
            }
        }
    }

    fn reject_macros(&self, annotations: &mut Vec<Annotation>, target: &str) {
        for invocation in self.take_invocations(annotations) {
            self.misplaced(&invocation, target);
        }
    }

    fn take_invocations(&self, annotations: &mut Vec<Annotation>) -> Vec<Invocation> {
        let mut invocations = Vec::new();
        annotations.retain(|annotation| {
            if is_builtin_attribute(annotation) {
                return true;
            }
            match self.scope.resolve(&annotation.path) {
                Some(builtin) => invocations.push(Invocation {
                    builtin,
                    annotation: annotation.clone(),
                }),
                None => self.emit(
                    Diagnostic::error(format!(
                        "cannot find macro `{}` in this scope",
                        annotation.path
                    ))
                    .with_span(annotation.path.span)
                    .with_label("not found in this scope"),
                ),
            }
            false
        });
        invocations
    }

    fn emit(&self, diagnostic: Diagnostic) {
        self.dcx.emit(diagnostic);
    }
}

fn is_builtin_attribute(annotation: &Annotation) -> bool {
    BUILTIN_ATTRIBUTES.iter().any(|&name| annotation.is(name))
}

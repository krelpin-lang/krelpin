use krelpin_ast::ast::{
    FieldStorage, GenericParamKind, Item, ItemKind, Member, MemberKind, Ty, TyKind,
};
use krelpin_ast::token::Token;
use krelpin_errors::Diagnostic;
use krelpin_parse::{lexer, parser};
use krelpin_span::{Expansion, Symbol};

use crate::{ExpandCtxt, Invocation};

pub(crate) struct Field {
    pub(crate) name: Symbol,
    pub(crate) ty: String,
    pub(crate) nullable: bool,
}

pub(crate) struct Variant {
    pub(crate) name: Symbol,
    pub(crate) fields: Vec<Field>,
}

pub(crate) enum Layout {
    Fields(Vec<Field>),
    Variants(Vec<Variant>),
}

pub(crate) struct ItemModel {
    pub(crate) self_ty: String,
    pub(crate) layout: Layout,
}

impl ItemModel {
    pub(crate) fn has_data(&self) -> bool {
        match &self.layout {
            Layout::Fields(_) => true,
            Layout::Variants(variants) => variants.iter().any(|variant| !variant.fields.is_empty()),
        }
    }
}

pub(crate) struct Generated {
    pub(crate) interfaces: Vec<String>,
    pub(crate) members: String,
}

impl ExpandCtxt<'_> {
    pub(crate) fn item_model(&self, item: &Item, invocation: &Invocation) -> Option<ItemModel> {
        let (generics, layout) = match &item.kind {
            ItemKind::Class(class) => {
                (&class.generics, Layout::Fields(self.fields(&class.members)))
            }
            ItemKind::Struct(def) => (&def.generics, Layout::Fields(self.fields(&def.members))),
            ItemKind::Enum(def) => {
                let variants = def
                    .variants
                    .iter()
                    .map(|variant| Variant {
                        name: variant.name.name,
                        fields: variant
                            .fields
                            .iter()
                            .map(|field| self.field(field.name.name, &field.ty))
                            .collect(),
                    })
                    .collect();
                (&def.generics, Layout::Variants(variants))
            }
            ItemKind::Interface(_) => {
                self.misplaced(invocation, "interfaces");
                return None;
            }
        };

        let mut params = Vec::new();
        for param in &generics.params {
            match param.kind {
                GenericParamKind::Type { .. } => params.push(param.name.name.as_str()),
                GenericParamKind::BorrowSource => {
                    self.emit(
                        Diagnostic::error(format!(
                            "`@{}` cannot be used on types with borrow sources",
                            invocation.builtin.name()
                        ))
                        .with_span(invocation.annotation.span)
                        .with_note("macros do not support borrowed data yet"),
                    );
                    return None;
                }
            }
        }
        let name = item.name.name;
        let self_ty = if params.is_empty() {
            name.as_str().to_owned()
        } else {
            format!("{name}<{}>", params.join(", "))
        };
        Some(ItemModel { self_ty, layout })
    }

    pub(crate) fn misplaced(&self, invocation: &Invocation, target: &str) {
        self.emit(
            Diagnostic::error(format!(
                "`@{}` cannot be used on {target}",
                invocation.builtin.name()
            ))
            .with_span(invocation.annotation.span)
            .with_help(invocation.builtin.usage()),
        );
    }

    pub(crate) fn splice(&self, item: &mut Item, invocation: &Invocation, generated: Generated) {
        let expansion = Expansion {
            call_site: invocation.annotation.span,
            macro_name: invocation.builtin.name().to_owned(),
        };
        let file_name = format!("<@{} on {}>", invocation.builtin.name(), item.name.name);
        let interfaces = if generated.interfaces.is_empty() {
            Vec::new()
        } else {
            let tokens =
                self.lex_generated(&file_name, generated.interfaces.join(", "), &expansion);
            parser::parse_tys(tokens, self.dcx).unwrap_or_default()
        };
        let tokens = self.lex_generated(&file_name, generated.members, &expansion);
        let members = parser::parse_members(tokens, item.name, self.dcx);

        let (implements, item_members): (&mut Vec<Ty>, &mut Vec<Member>) = match &mut item.kind {
            ItemKind::Class(class) => (&mut class.implements, &mut class.members),
            ItemKind::Struct(def) => (&mut def.implements, &mut def.members),
            ItemKind::Enum(def) => (&mut def.implements, &mut def.members),
            ItemKind::Interface(_) => return,
        };
        implements.extend(interfaces);
        item_members.extend(members);
    }

    fn lex_generated(&self, name: &str, src: String, expansion: &Expansion) -> Vec<Token> {
        let file = self
            .source_map
            .new_expansion_file(name, src, expansion.clone());
        lexer::lex(&file, self.dcx)
    }

    fn fields(&self, members: &[Member]) -> Vec<Field> {
        members
            .iter()
            .filter_map(|member| match &member.kind {
                MemberKind::Field(field) if field.storage == FieldStorage::Instance => {
                    Some(self.field(field.name.name, &field.ty))
                }
                _ => None,
            })
            .collect()
    }

    fn field(&self, name: Symbol, ty: &Ty) -> Field {
        Field {
            name,
            ty: self
                .source_map
                .span_to_snippet(ty.span)
                .unwrap_or_else(|| ty.to_string()),
            nullable: matches!(ty.kind, TyKind::Nullable(_)),
        }
    }
}

pub(crate) fn variant_pattern(variant: &Variant, prefix: &str) -> String {
    if variant.fields.is_empty() {
        return variant.name.as_str().to_owned();
    }
    let bindings: Vec<String> = variant
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| format!("{} {prefix}{index}", field.ty))
        .collect();
    format!("{}({})", variant.name, bindings.join(", "))
}

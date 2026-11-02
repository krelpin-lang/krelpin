use std::fmt::Write;

use krelpin_ast::ast::{
    Annotation, FieldStorage, GenericParamKind, Item, ItemKind, Member, MemberKind, Ty, TyKind,
};
use krelpin_ast::token::{Token, TokenKind};
use krelpin_errors::Diagnostic;
use krelpin_parse::{lexer, parser};
use krelpin_span::{Expansion, Span, Symbol};

use crate::ExpandCtxt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Derivable {
    Equatable,
    Hashable,
}

impl Derivable {
    const ALL: &[Derivable] = &[Derivable::Equatable, Derivable::Hashable];

    fn name(self) -> &'static str {
        match self {
            Derivable::Equatable => "Equatable",
            Derivable::Hashable => "Hashable",
        }
    }

    fn from_name(name: Symbol) -> Option<Derivable> {
        Self::ALL
            .iter()
            .copied()
            .find(|derivable| derivable.name() == name.as_str())
    }
}

struct FieldValue {
    ty: String,
    nullable: bool,
}

enum Layout {
    Fields(Vec<(Symbol, FieldValue)>),
    Variants(Vec<(Symbol, Vec<FieldValue>)>),
}

struct Generated {
    interfaces: String,
    members: String,
}

impl ExpandCtxt<'_> {
    pub(crate) fn expand_derive(&self, item: &mut Item, annotation: &Annotation) {
        let Some(derived) = self.derive_args(annotation) else {
            return;
        };
        let Some(self_ty) = self.self_ty(item, annotation.span) else {
            return;
        };
        let Some(layout) = self.layout(item, annotation.span) else {
            return;
        };

        let generated = generate(&self_ty, &layout, &derived);
        let expansion = Expansion {
            call_site: annotation.span,
            macro_name: "Derive".to_owned(),
        };
        let file_name = format!("<@Derive on {}>", item.name.name);
        let interfaces = self.parse_generated(&file_name, generated.interfaces, &expansion);
        let interfaces = parser::parse_tys(interfaces, self.dcx).unwrap_or_default();
        let members = self.parse_generated(&file_name, generated.members, &expansion);
        let members = parser::parse_members(members, item.name, self.dcx);

        let (implements, item_members): (&mut Vec<Ty>, &mut Vec<Member>) = match &mut item.kind {
            ItemKind::Class(class) => (&mut class.implements, &mut class.members),
            ItemKind::Struct(def) => (&mut def.implements, &mut def.members),
            ItemKind::Enum(def) => (&mut def.implements, &mut def.members),
            ItemKind::Interface(_) => return,
        };
        implements.extend(interfaces);
        item_members.extend(members);
    }

    fn parse_generated(&self, name: &str, src: String, expansion: &Expansion) -> Vec<Token> {
        let file = self
            .source_map
            .new_expansion_file(name, src, expansion.clone());
        lexer::lex(&file, self.dcx)
    }

    fn derive_args(&self, annotation: &Annotation) -> Option<Vec<Derivable>> {
        let tokens = annotation
            .args
            .as_ref()
            .map_or(&[][..], |args| args.tokens.as_slice());
        if tokens.is_empty() {
            self.emit(
                Diagnostic::error("`@Derive` needs the interfaces to implement")
                    .with_span(annotation.span)
                    .with_help("list them, like `@Derive(Equatable, Hashable)`"),
            );
            return None;
        }

        let mut derived = Vec::new();
        let mut ok = true;
        for (index, token) in tokens.iter().enumerate() {
            let expects_name = index % 2 == 0;
            match token.kind {
                TokenKind::Ident(name) if expects_name => match Derivable::from_name(name) {
                    Some(derivable) if derived.contains(&derivable) => {
                        self.emit(
                            Diagnostic::error(format!("`{name}` is derived more than once"))
                                .with_span(token.span),
                        );
                        ok = false;
                    }
                    Some(derivable) => derived.push(derivable),
                    None => {
                        self.emit(
                            Diagnostic::error(format!("cannot derive `{name}`"))
                                .with_span(token.span)
                                .with_note("`@Derive` can implement `Equatable` and `Hashable`"),
                        );
                        ok = false;
                    }
                },
                TokenKind::Comma if !expects_name => {}
                _ => {
                    self.emit(
                        Diagnostic::error("expected the name of an interface")
                            .with_span(token.span)
                            .with_help("list the interfaces, like `@Derive(Equatable, Hashable)`"),
                    );
                    return None;
                }
            }
        }
        ok.then_some(derived)
    }

    fn self_ty(&self, item: &Item, span: Span) -> Option<String> {
        let generics = match &item.kind {
            ItemKind::Class(class) => &class.generics,
            ItemKind::Struct(def) => &def.generics,
            ItemKind::Enum(def) => &def.generics,
            ItemKind::Interface(_) => {
                self.emit(
                    Diagnostic::error("`@Derive` cannot be used on interfaces")
                        .with_span(span)
                        .with_help("`@Derive` can only be used on classes, structs and enums"),
                );
                return None;
            }
        };
        let mut params = Vec::new();
        for param in &generics.params {
            match param.kind {
                GenericParamKind::Type { .. } => params.push(param.name.name.as_str()),
                GenericParamKind::BorrowSource => {
                    self.emit(
                        Diagnostic::error("`@Derive` cannot be used on types with borrow sources")
                            .with_span(span)
                            .with_note("deriving for borrowed data is not supported yet"),
                    );
                    return None;
                }
            }
        }
        let name = item.name.name;
        Some(if params.is_empty() {
            name.as_str().to_owned()
        } else {
            format!("{name}<{}>", params.join(", "))
        })
    }

    fn layout(&self, item: &Item, span: Span) -> Option<Layout> {
        match &item.kind {
            ItemKind::Class(class) => Some(Layout::Fields(self.fields(&class.members))),
            ItemKind::Struct(def) => Some(Layout::Fields(self.fields(&def.members))),
            ItemKind::Enum(def) => {
                if def.variants.iter().all(|variant| variant.fields.is_empty()) {
                    self.emit(
                        Diagnostic::error(
                            "enums without data are already `Equatable` and `Hashable`",
                        )
                        .with_span(span)
                        .with_help("remove the `@Derive`"),
                    );
                    return None;
                }
                let variants = def
                    .variants
                    .iter()
                    .map(|variant| {
                        let fields = variant
                            .fields
                            .iter()
                            .map(|field| self.value(&field.ty))
                            .collect();
                        (variant.name.name, fields)
                    })
                    .collect();
                Some(Layout::Variants(variants))
            }
            ItemKind::Interface(_) => None,
        }
    }

    fn fields(&self, members: &[Member]) -> Vec<(Symbol, FieldValue)> {
        members
            .iter()
            .filter_map(|member| match &member.kind {
                MemberKind::Field(field) if field.storage == FieldStorage::Instance => {
                    Some((field.name.name, self.value(&field.ty)))
                }
                _ => None,
            })
            .collect()
    }

    fn value(&self, ty: &Ty) -> FieldValue {
        FieldValue {
            ty: self
                .source_map
                .span_to_snippet(ty.span)
                .unwrap_or_else(|| ty.to_string()),
            nullable: matches!(ty.kind, TyKind::Nullable(_)),
        }
    }
}

fn generate(self_ty: &str, layout: &Layout, derived: &[Derivable]) -> Generated {
    let mut interfaces = Vec::new();
    let mut members = String::new();
    for &derivable in derived {
        interfaces.push(format!("{}<{self_ty}>", derivable.name()));
        let method = match (derivable, layout) {
            (Derivable::Equatable, Layout::Fields(fields)) => fields_equals(self_ty, fields),
            (Derivable::Equatable, Layout::Variants(variants)) => {
                variants_equals(self_ty, variants)
            }
            (Derivable::Hashable, Layout::Fields(fields)) => fields_hash_code(fields),
            (Derivable::Hashable, Layout::Variants(variants)) => variants_hash_code(variants),
        };
        members.push_str(&method);
    }
    Generated {
        interfaces: interfaces.join(", "),
        members,
    }
}

fn fields_equals(self_ty: &str, fields: &[(Symbol, FieldValue)]) -> String {
    let comparisons: Vec<String> = fields
        .iter()
        .map(|(name, _)| format!("this.{name} == other.{name}"))
        .collect();
    let body = if comparisons.is_empty() {
        "true".to_owned()
    } else {
        comparisons.join("\n            && ")
    };
    format!(
        "public boolean equals(&this, &{self_ty} other) {{\n    \
         return {body};\n\
         }}\n\n"
    )
}

fn fields_hash_code(fields: &[(Symbol, FieldValue)]) -> String {
    let mut body = String::from("    int hash = 17;\n");
    for (name, value) in fields {
        let hash = hash_of(&format!("this.{name}"), value);
        let _ = writeln!(body, "    hash = hash * 31 + {hash};");
    }
    format!("public int hashCode(&this) {{\n{body}    return hash;\n}}\n\n")
}

fn variants_equals(self_ty: &str, variants: &[(Symbol, Vec<FieldValue>)]) -> String {
    let mut arms = String::new();
    for (name, values) in variants {
        let this_pattern = pattern(name, values, "a");
        let other_pattern = pattern(name, values, "b");
        let comparisons: Vec<String> = std::iter::once(format!("other instanceof {other_pattern}"))
            .chain((0..values.len()).map(|index| format!("a{index} == b{index}")))
            .collect();
        let _ = writeln!(
            arms,
            "        case {this_pattern} -> {};",
            comparisons.join(" && ")
        );
    }
    format!(
        "public boolean equals(&this, &{self_ty} other) {{\n    \
         return switch (this) {{\n{arms}    }};\n\
         }}\n\n"
    )
}

fn variants_hash_code(variants: &[(Symbol, Vec<FieldValue>)]) -> String {
    let mut arms = String::new();
    for (name, values) in variants {
        let hash =
            values
                .iter()
                .enumerate()
                .fold("this.ordinal()".to_owned(), |hash, (index, value)| {
                    format!("({hash}) * 31 + {}", hash_of(&format!("a{index}"), value))
                });
        let _ = writeln!(
            arms,
            "        case {} -> {hash};",
            pattern(name, values, "a")
        );
    }
    format!(
        "public int hashCode(&this) {{\n    \
         return switch (this) {{\n{arms}    }};\n\
         }}\n\n"
    )
}

fn pattern(variant: &Symbol, values: &[FieldValue], prefix: &str) -> String {
    if values.is_empty() {
        return variant.as_str().to_owned();
    }
    let bindings: Vec<String> = values
        .iter()
        .enumerate()
        .map(|(index, value)| format!("{} {prefix}{index}", value.ty))
        .collect();
    format!("{variant}({})", bindings.join(", "))
}

fn hash_of(expr: &str, value: &FieldValue) -> String {
    if value.nullable {
        format!("({expr}?.hashCode() ?: 0)")
    } else {
        format!("{expr}.hashCode()")
    }
}

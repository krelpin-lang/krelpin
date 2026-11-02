use std::fmt::Write;

use krelpin_ast::ast::Item;
use krelpin_ast::token::TokenKind;
use krelpin_errors::Diagnostic;
use krelpin_span::Symbol;

use crate::item::{Field, Generated, ItemModel, Layout, Variant, variant_pattern};
use crate::{ExpandCtxt, Invocation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Derivable {
    Equatable,
    Hashable,
    Comparable,
}

impl Derivable {
    const ALL: &[Derivable] = &[
        Derivable::Equatable,
        Derivable::Hashable,
        Derivable::Comparable,
    ];

    fn name(self) -> &'static str {
        match self {
            Derivable::Equatable => "Equatable",
            Derivable::Hashable => "Hashable",
            Derivable::Comparable => "Comparable",
        }
    }

    fn from_name(name: Symbol) -> Option<Derivable> {
        Self::ALL
            .iter()
            .copied()
            .find(|derivable| derivable.name() == name.as_str())
    }
}

impl ExpandCtxt<'_> {
    pub(crate) fn expand_derive(&self, item: &mut Item, invocation: &Invocation) {
        let Some(derived) = self.derive_args(invocation) else {
            return;
        };
        let Some(model) = self.item_model(item, invocation) else {
            return;
        };
        let built_in = derived
            .iter()
            .find(|derivable| **derivable != Derivable::Comparable);
        if let (false, Some(&derivable)) = (model.has_data(), built_in) {
            self.emit(
                Diagnostic::error(format!(
                    "enums without data are already `{}`",
                    derivable.name()
                ))
                .with_span(invocation.annotation.span)
                .with_note("enums without data are always `Equatable` and `Hashable`"),
            );
            return;
        }
        if derived.contains(&Derivable::Comparable) && !self.check_orderable(&model, invocation) {
            return;
        }

        let mut generated = Generated {
            interfaces: Vec::new(),
            members: String::new(),
        };
        for derivable in derived {
            generated
                .interfaces
                .push(format!("{}<{}>", derivable.name(), model.self_ty));
            let method = match (derivable, &model.layout) {
                (Derivable::Equatable, Layout::Fields(fields)) => {
                    fields_equals(&model.self_ty, fields)
                }
                (Derivable::Equatable, Layout::Variants(variants)) => {
                    variants_equals(&model.self_ty, variants)
                }
                (Derivable::Hashable, Layout::Fields(fields)) => fields_hash_code(fields),
                (Derivable::Hashable, Layout::Variants(variants)) => variants_hash_code(variants),
                (Derivable::Comparable, Layout::Fields(fields)) => {
                    fields_compare_to(&model.self_ty, fields)
                }
                (Derivable::Comparable, Layout::Variants(variants)) => {
                    variants_compare_to(&model.self_ty, variants)
                }
            };
            generated.members.push_str(&method);
        }
        self.splice(item, invocation, generated);
    }

    fn check_orderable(&self, model: &ItemModel, invocation: &Invocation) -> bool {
        let nullable = match &model.layout {
            Layout::Fields(fields) => fields.iter().find(|field| field.nullable),
            Layout::Variants(variants) => variants
                .iter()
                .flat_map(|variant| &variant.fields)
                .find(|field| field.nullable),
        };
        let Some(field) = nullable else {
            return true;
        };
        self.emit(
            Diagnostic::error(format!(
                "`Comparable` cannot be derived because `{}` may be null",
                field.name
            ))
            .with_span(invocation.annotation.span)
            .with_help("implement `compareTo` yourself to decide where null values go"),
        );
        false
    }

    fn derive_args(&self, invocation: &Invocation) -> Option<Vec<Derivable>> {
        let annotation = &invocation.annotation;
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
                                .with_note(
                                    "`@Derive` can implement `Equatable`, `Hashable` \
                                     and `Comparable`",
                                ),
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
}

fn fields_equals(self_ty: &str, fields: &[Field]) -> String {
    let comparisons: Vec<String> = fields
        .iter()
        .map(|field| format!("this.{name} == other.{name}", name = field.name))
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

fn fields_hash_code(fields: &[Field]) -> String {
    let mut body = String::from("    int hash = 17;\n");
    for field in fields {
        let hash = hash_of(&format!("this.{}", field.name), field);
        let _ = writeln!(body, "    hash = hash * 31 + {hash};");
    }
    format!("public int hashCode(&this) {{\n{body}    return hash;\n}}\n\n")
}

fn fields_compare_to(self_ty: &str, fields: &[Field]) -> String {
    let mut body = String::new();
    for field in fields {
        let _ = write!(
            body,
            "    order = this.{name}.compareTo(&other.{name});\n    \
             if (order != 0) {{\n        return order;\n    }}\n",
            name = field.name
        );
    }
    format!(
        "public int compareTo(&this, &{self_ty} other) {{\n    \
         int order = 0;\n{body}    return order;\n\
         }}\n\n"
    )
}

fn variants_equals(self_ty: &str, variants: &[Variant]) -> String {
    let mut arms = String::new();
    for variant in variants {
        let comparisons: Vec<String> = std::iter::once(format!(
            "other instanceof {}",
            variant_pattern(variant, "b")
        ))
        .chain((0..variant.fields.len()).map(|index| format!("a{index} == b{index}")))
        .collect();
        let _ = writeln!(
            arms,
            "        case {} -> {};",
            variant_pattern(variant, "a"),
            comparisons.join(" && ")
        );
    }
    format!(
        "public boolean equals(&this, &{self_ty} other) {{\n    \
         return switch (this) {{\n{arms}    }};\n\
         }}\n\n"
    )
}

fn variants_hash_code(variants: &[Variant]) -> String {
    let mut arms = String::new();
    for variant in variants {
        let hash = variant.fields.iter().enumerate().fold(
            "this.ordinal()".to_owned(),
            |hash, (index, field)| {
                format!("({hash}) * 31 + {}", hash_of(&format!("a{index}"), field))
            },
        );
        let _ = writeln!(
            arms,
            "        case {} -> {hash};",
            variant_pattern(variant, "a")
        );
    }
    format!(
        "public int hashCode(&this) {{\n    \
         return switch (this) {{\n{arms}    }};\n\
         }}\n\n"
    )
}

fn variants_compare_to(self_ty: &str, variants: &[Variant]) -> String {
    let mut arms = String::new();
    for variant in variants {
        let mut body = String::new();
        if !variant.fields.is_empty() {
            let _ = writeln!(
                body,
                "            if (other instanceof {}) {{",
                variant_pattern(variant, "b")
            );
            for index in 0..variant.fields.len() {
                let _ = write!(
                    body,
                    "                order = a{index}.compareTo(&b{index});\n                \
                     if (order != 0) {{\n                    return order;\n                }}\n"
                );
            }
            body.push_str("            }\n");
        }
        let _ = write!(
            arms,
            "        case {} -> {{\n{body}        }}\n",
            variant_pattern(variant, "a")
        );
    }
    format!(
        "public int compareTo(&this, &{self_ty} other) {{\n    \
         int order = this.ordinal().compareTo(&other.ordinal());\n    \
         if (order != 0) {{\n        return order;\n    }}\n    \
         switch (this) {{\n{arms}    }}\n    \
         return 0;\n\
         }}\n\n"
    )
}

fn hash_of(expr: &str, field: &Field) -> String {
    if field.nullable {
        format!("({expr}?.hashCode() ?: 0)")
    } else {
        format!("{expr}.hashCode()")
    }
}

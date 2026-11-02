use std::fmt::Write;

use krelpin_ast::ast::Item;

use krelpin_span::Symbol;

use crate::item::{Generated, Layout, variant_pattern};
use crate::{ExpandCtxt, Invocation};

impl ExpandCtxt<'_> {
    pub(crate) fn expand_to_string(&self, item: &mut Item, invocation: &Invocation) {
        if self.reject_args(invocation) {
            return;
        }
        let Some(model) = self.item_model(item, invocation) else {
            return;
        };
        let body = match &model.layout {
            Layout::Fields(fields) => {
                let parts = describe(
                    fields
                        .iter()
                        .map(|field| (field.name, format!("this.{}", field.name))),
                );
                format!("    return \"{}({parts})\";\n", model.name)
            }
            Layout::Variants(variants) => {
                let mut arms = String::new();
                for variant in variants {
                    let text = if variant.fields.is_empty() {
                        format!("\"{}\"", variant.name)
                    } else {
                        let parts = describe(
                            variant
                                .fields
                                .iter()
                                .enumerate()
                                .map(|(index, field)| (field.name, format!("a{index}"))),
                        );
                        format!("\"{}({parts})\"", variant.name)
                    };
                    let _ = writeln!(
                        arms,
                        "        case {} -> {text};",
                        variant_pattern(variant, "a")
                    );
                }
                format!("    return switch (this) {{\n{arms}    }};\n")
            }
        };
        let generated = Generated {
            interfaces: Vec::new(),
            members: format!("public String toString(&this) {{\n{body}}}\n"),
        };
        self.splice(item, invocation, generated);
    }
}

fn describe(fields: impl Iterator<Item = (Symbol, String)>) -> String {
    let parts: Vec<String> = fields
        .map(|(name, value)| format!("{name}=\" + {value} + \""))
        .collect();
    parts.join(", ")
}

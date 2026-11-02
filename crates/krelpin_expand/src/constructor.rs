use std::fmt::Write;

use krelpin_ast::ast::Item;

use crate::item::{Generated, Layout};
use crate::{ExpandCtxt, Invocation};

impl ExpandCtxt<'_> {
    pub(crate) fn expand_constructor(&self, item: &mut Item, invocation: &Invocation) {
        if self.reject_args(invocation) {
            return;
        }
        let Some(model) = self.item_model(item, invocation) else {
            return;
        };
        let Layout::Fields(fields) = &model.layout else {
            self.misplaced(invocation, "enums");
            return;
        };

        let fields: Vec<_> = fields.iter().filter(|field| !field.initialized).collect();
        let params: Vec<String> = fields
            .iter()
            .map(|field| format!("{} {}", field.ty, field.name))
            .collect();
        let mut body = String::new();
        for field in &fields {
            let _ = writeln!(body, "    this.{name} = {name};", name = field.name);
        }
        let generated = Generated {
            interfaces: Vec::new(),
            members: format!("{}({}) {{\n{body}}}\n", model.name, params.join(", ")),
        };
        self.splice(item, invocation, generated);
    }
}

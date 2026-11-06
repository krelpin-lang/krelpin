//! Accessors that a record declares itself.
//!
//! Every component of a record gets an accessor of the same name. A record
//! may write one itself, to compute or format the value, and it then
//! replaces the generated one. It must look exactly like the generated
//! accessor, so callers can't tell the two apart: a visible instance method
//! that takes nothing, has no type parameters, throws nothing and returns
//! what the generated accessor would.

use krelpin_ast::ast::{Member, Method, ReceiverKind, Visibility};
use krelpin_errors::Diagnostic;

use crate::check::FnCtxt;
use crate::sig::AdtKind;
use crate::ty::Ty;

impl FnCtxt<'_, '_> {
    pub(crate) fn check_record_accessor(&self, member: &Member, method: &Method, ret: &Ty) {
        let sig = self.tcx.sig(self.owner);
        let name = method.name.name;
        if sig.kind != AdtKind::Record
            || !method.params.is_empty()
            || !sig.components.contains(&name)
        {
            return;
        }
        let component = sig.fields[&name].ty.clone();
        let expected = self.tcx.accessor_ty(&component);
        let accessor = |problem: String| {
            Diagnostic::error(format!("the accessor `{name}` {problem}"))
                .with_span(method.name.span)
                .with_note(format!(
                    "`{name}()` replaces the accessor of the component `{name}`, so it must \
                     look like it: `{expected} {name}()`"
                ))
        };

        match method.receiver.map(|receiver| receiver.kind) {
            None => self.emit(
                accessor("can't be `static`".to_owned())
                    .with_help("remove `static`; an accessor reads the record it is called on"),
            ),
            Some(ReceiverKind::RefMut) => self.emit(
                accessor("can't be `mut`".to_owned())
                    .with_note("the components of a record never change"),
            ),
            Some(ReceiverKind::Owned) => self.emit(
                accessor("can't take the record by value".to_owned())
                    .with_help("read it through `this` like other methods"),
            ),
            Some(ReceiverKind::Ref) => {}
        }
        if let Visibility::Private(span) | Visibility::Protected(span) = member.vis {
            self.emit(
                accessor("must be visible wherever the record is".to_owned())
                    .with_span(span)
                    .with_help("remove the visibility modifier"),
            );
        }
        if let Some(param) = method.generics.params.first() {
            self.emit(accessor("can't have type parameters".to_owned()).with_span(param.name.span));
        }
        if let Some(thrown) = method.throws.first() {
            self.emit(
                accessor("can't throw".to_owned())
                    .with_span(thrown.span)
                    .with_help("reading a component never fails; check values in the constructor"),
            );
        }
        if let Some(span) = method.asyncness {
            self.emit(accessor("can't be `async`".to_owned()).with_span(span));
        }
        if !ret.references_unknown() && *ret != expected {
            let help = if self.tcx.is_copy(&component) {
                format!("return `{expected}`, the type of the component")
            } else {
                format!(
                    "return `{expected}`: a `{component}` can't be copied, so the accessor \
                     lends it out"
                )
            };
            self.emit(
                accessor(format!("must return `{expected}`, not `{ret}`"))
                    .with_span(method_ret_span(method).unwrap_or(method.name.span))
                    .with_help(help),
            );
        }
    }
}

fn method_ret_span(method: &Method) -> Option<krelpin_span::Span> {
    match &method.ret {
        krelpin_ast::ast::FnRetTy::Void(span) => Some(*span),
        krelpin_ast::ast::FnRetTy::Ty(ty) => Some(ty.span),
    }
}

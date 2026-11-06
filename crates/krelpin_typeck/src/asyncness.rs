//! `async` methods and `await`.

use krelpin_ast::ast::{Expr, ExprKind, Method};
use krelpin_errors::Diagnostic;
use krelpin_span::{Span, Symbol};

use crate::check::FnCtxt;
use crate::lang_items::LangItem;
use crate::ty::Ty;

impl FnCtxt<'_, '_> {
    pub(crate) fn check_async_method(&self, method: &Method, span: Span) {
        if method.body.is_none() {
            self.emit(
                Diagnostic::error("an `async` method needs a body")
                    .with_span(span)
                    .with_help(format!(
                        "to declare one without a body, return a future instead: \
                         `Future<...> {}(...)`",
                        method.name.name
                    )),
            );
        }
        if let Some(first) = method.borrows.first() {
            self.emit(
                Diagnostic::error("an `async` method can't name what it borrows")
                    .with_span(first.span)
                    .with_note("its future holds every argument, so it borrows all of them"),
            );
        }
        if self.tcx.lang_items.get(LangItem::Future).is_none() {
            self.error(span, "`async` needs `std.task.Future`, which is missing");
        }
    }

    /// `await future`: the future's value. A future made by calling an
    /// `async` method throws what that method declares; any other throws
    /// what its `poll` declares.
    pub(crate) fn check_await(&mut self, operand: &Expr, span: Span) -> Ty {
        if !self.in_async {
            self.emit(
                Diagnostic::error("`await` is only allowed inside an `async` method")
                    .with_span(span)
                    .with_help("mark the method `async`; a lambda can't be `async`"),
            );
        }
        let ty = self.check_expr(operand);
        if ty.is_unknown() {
            return Ty::Unknown;
        }
        let Some(output) = self.tcx.future_output(&ty) else {
            let note = match &ty {
                Ty::Nullable(inner) if self.tcx.future_output(inner).is_some() => {
                    "check that it isn't `null` first"
                }
                Ty::Ref { ty, .. } if self.tcx.future_output(ty).is_some() => {
                    "`await` takes the future itself, not a reference to it"
                }
                _ => "only a `Future` can be awaited",
            };
            self.emit(
                Diagnostic::error(format!("cannot `await` a value of type `{ty}`"))
                    .with_span(operand.span)
                    .with_note(note),
            );
            return Ty::Unknown;
        };
        for thrown in self.await_throws(operand, &ty) {
            self.raise(thrown, span);
        }
        output
    }

    fn await_throws(&self, operand: &Expr, ty: &Ty) -> Vec<Ty> {
        if let ExprKind::MethodCall { name, .. } = &operand.kind
            && let Some(callee) = self.results.callees.get(&name.span)
            && callee.method.output.is_some()
        {
            return callee
                .method
                .throws
                .iter()
                .map(|thrown| thrown.subst(&callee.subst))
                .collect();
        }
        let Ty::Adt { def, args, .. } = ty else {
            return Vec::new();
        };
        let poll = Symbol::intern("poll");
        self.tcx
            .methods_named(*def, args, poll)
            .into_iter()
            .find(|(method, _)| method.params.len() == 1 && method.receiver.is_some())
            .map(|(method, subst)| {
                method
                    .throws
                    .iter()
                    .map(|thrown| thrown.subst(&subst))
                    .collect()
            })
            .unwrap_or_default()
    }
}

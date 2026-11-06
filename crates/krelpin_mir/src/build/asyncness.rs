//! The poll of an `async` method.
//!
//! The body of an `async` method is built as the `poll` of its future. Its
//! locals live in the future, so they keep their values between polls; the
//! code generator puts them there. `return value` becomes
//! `return Poll.Ready(value)`, and `await future` becomes a loop:
//!
//! ```text
//! loop:   result = Future.poll(&mut future, cx)
//!         switch discriminant(result) { Ready => ready, _ => pending }
//! pending: _0 = Poll.Pending
//!         suspend -> loop
//! ready:  dest = (result as Ready).value
//! ```

use krelpin_ast::ast::ReceiverKind;
use krelpin_hir::{Dispatch, Expr};
use krelpin_span::{Span, Symbol};
use krelpin_typeck::{Callee, LangItem, Ty};

use super::Builder;
use crate::mir::{AggregateKind, Local, Operand, Place, PlaceElem, Rvalue, TerminatorKind};

const READY: &str = "Ready";
const PENDING: &str = "Pending";

impl<'h> Builder<'_, 'h, '_> {
    fn poll_variant(&self, name: &str) -> (krelpin_span::def_id::DefId, usize) {
        let poll = self
            .tcx()
            .lang_items
            .get(LangItem::Poll)
            .expect("`async` bodies are only built when `std.task.Poll` exists");
        let index = self
            .variant_index(poll, Symbol::intern(name))
            .unwrap_or_else(|| panic!("`Poll` has no variant `{name}`"));
        (poll, index)
    }

    /// `return value;` (or the end of an `async void` body): finishes the
    /// future with `Poll.Ready(value)`.
    pub(super) fn async_return(&mut self, value: Option<&'h Expr>, span: Span) {
        let state = self
            .asyncness
            .clone()
            .expect("only called in `async` bodies");
        let slot = Place::local(state.value);
        match value {
            Some(value) => self.expr_into(slot.clone(), value),
            None => {
                let unit = self.tcx().lang_ty(LangItem::Unit);
                let Ty::Adt { def, .. } = unit else {
                    panic!("`std.task.Unit` is missing");
                };
                self.assign(
                    slot.clone(),
                    Rvalue::Aggregate(
                        Box::new(AggregateKind::Record { def, ty: unit }),
                        Vec::new(),
                    ),
                    span,
                );
            }
        }
        self.exit_scopes(0, span);
        let (poll, ready) = self.poll_variant(READY);
        self.assign(
            Place::local(Local::RETURN),
            Rvalue::Aggregate(
                Box::new(AggregateKind::Variant {
                    def: poll,
                    index: ready,
                    ty: state.poll,
                }),
                vec![Operand::Move(slot)],
            ),
            span,
        );
        self.diverge(TerminatorKind::Return, span);
    }

    pub(super) fn lower_await(&mut self, dest: Place, future: &'h Expr, span: Span) {
        let state = self
            .asyncness
            .clone()
            .expect("`await` is only allowed in `async` bodies");
        let tcx = self.tcx();
        let future_ty = future.ty.clone();
        let output = tcx
            .future_output(&future_ty)
            .expect("the type checker only lets futures be awaited");
        let future_def = tcx
            .lang_items
            .get(LangItem::Future)
            .expect("futures need `std.task.Future`");
        let (method, subst) = tcx
            .methods_named(
                future_def,
                std::slice::from_ref(&output),
                Symbol::intern("poll"),
            )
            .into_iter()
            .find(|(method, _)| method.params.len() == 1)
            .expect("`Future` has a `poll` method");
        let callee = Callee {
            params: method
                .params
                .iter()
                .map(|param| param.subst(&subst))
                .collect(),
            ret: method.ret.subst(&subst),
            packs_varargs: false,
            subst,
            method,
        };
        let result_ty = callee.ret.clone();

        let held = self.temp(&future_ty, span);
        self.expr_into(Place::local(held), future);
        let poll_again = self.goto_new(span);
        let receiver = self.receiver_operand(
            Place::local(held),
            &future_ty,
            Some(ReceiverKind::RefMut),
            span,
        );
        let context_ty = self.locals[state.body.cx.index()].ty.clone();
        let context = self.operand_from_place(Place::local(state.body.cx), &context_ty, span);
        let result = self.temp(&result_ty, span);
        self.call(
            &callee,
            Dispatch::Virtual,
            vec![receiver, context],
            Some(Place::local(result)),
            span,
        );

        let (poll, ready) = self.poll_variant(READY);
        let (_, pending) = self.poll_variant(PENDING);
        let discriminant = self.temp(&Ty::INT, span);
        self.assign(
            Place::local(discriminant),
            Rvalue::Discriminant(Place::local(result)),
            span,
        );
        let is_ready = self.new_block();
        let not_ready = self.new_block();
        self.terminate(
            TerminatorKind::SwitchInt {
                discr: Operand::Move(Place::local(discriminant)),
                targets: vec![(ready as u64, is_ready)],
                otherwise: not_ready,
            },
            span,
        );

        self.current = not_ready;
        self.assign(
            Place::local(Local::RETURN),
            Rvalue::Aggregate(
                Box::new(AggregateKind::Variant {
                    def: poll,
                    index: pending,
                    ty: state.poll,
                }),
                Vec::new(),
            ),
            span,
        );
        self.terminate(TerminatorKind::Suspend { resume: poll_again }, span);

        self.current = is_ready;
        let value = Place::local(result)
            .project(PlaceElem::Variant {
                def: poll,
                index: ready,
            })
            .project(PlaceElem::Field {
                owner: poll,
                name: Symbol::intern("value"),
            });
        let value = self.operand_from_place(value, &output, span);
        self.assign(dest, Rvalue::Use(value), span);
    }
}

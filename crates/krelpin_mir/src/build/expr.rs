use std::collections::HashMap;

use krelpin_ast::ast::{BinOp, ReceiverKind, UnOp};
use krelpin_ast::token::{Lit, LitKind};
use krelpin_hir::{Closure, ClosureKind, Dispatch, Expr, ExprKind, WithField};
use krelpin_span::Span;
use krelpin_span::def_id::DefId;
use krelpin_typeck::{AdtKind, Callee, LangItem, Ty};

use super::Builder;
use crate::mir::{
    AggregateKind, BasicBlock, CaptureInfo, CaptureMode, Check, ClosureTarget, Constant, Operand,
    Place, PlaceBase, PlaceElem, Rvalue, TerminatorKind, UnaryOp,
};

pub(super) struct CallParts<'h> {
    pub(super) callee: &'h Callee,
    pub(super) receiver: Option<&'h Expr>,
    pub(super) args: &'h [Expr],
    pub(super) dispatch: Dispatch,
    pub(super) safe: bool,
}

impl<'h> Builder<'_, 'h, '_> {
    pub(super) fn expr_into(&mut self, dest: Place, expr: &'h Expr) {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Lit(_)
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Local(_)
            | ExprKind::Field { safe: false, .. }
            | ExprKind::Deref { safe: false, .. }
            | ExprKind::Index { .. } => {
                let operand = self.as_operand(expr);
                self.assign(dest, Rvalue::Use(operand), span);
            }
            ExprKind::Deref {
                expr: inner,
                safe: true,
            } => {
                let reference = self.as_place(inner);
                let (null, present) = self.null_branch(reference.clone(), span);
                let join = self.new_block();
                self.current = null;
                self.assign(dest.clone(), Rvalue::Use(Constant::Null.into()), span);
                self.goto(join, span);
                self.current = present;
                let operand = self.operand_from_place(
                    reference.project(PlaceElem::Deref),
                    expr.ty.non_null(),
                    span,
                );
                self.assign(dest, Rvalue::Use(operand), span);
                self.goto(join, span);
                self.current = join;
            }
            ExprKind::Static { owner, name } => match self.variant_index(*owner, *name) {
                Some(index) => self.assign(
                    dest,
                    Rvalue::Aggregate(
                        Box::new(AggregateKind::Variant {
                            def: *owner,
                            index,
                            ty: expr.ty.clone(),
                        }),
                        Vec::new(),
                    ),
                    span,
                ),
                None => {
                    let operand = self.as_operand(expr);
                    self.assign(dest, Rvalue::Use(operand), span);
                }
            },
            ExprKind::Field {
                base,
                owner,
                name,
                safe: true,
            } => {
                let base_place = self.as_place(base);
                let (null, present) = self.null_branch(base_place.clone(), span);
                let join = self.new_block();
                self.current = null;
                self.assign(dest.clone(), Rvalue::Use(Constant::Null.into()), span);
                self.goto(join, span);
                self.current = present;
                let field = deref(base_place, &base.ty).project(PlaceElem::Field {
                    owner: *owner,
                    name: *name,
                });
                let operand = self.operand_from_place(field, expr.ty.non_null(), span);
                self.assign(dest, Rvalue::Use(operand), span);
                self.goto(join, span);
                self.current = join;
            }
            ExprKind::Call {
                callee,
                receiver,
                args,
                dispatch,
                safe,
            } => self.lower_call(
                Some(dest),
                CallParts {
                    callee,
                    receiver: receiver.as_deref(),
                    args,
                    dispatch: *dispatch,
                    safe: *safe,
                },
                span,
            ),
            ExprKind::New { callee, args } => self.lower_new(dest, &expr.ty, callee, args, span),
            ExprKind::CtorCall { callee, this, args } => {
                self.lower_ctor_call(callee, this, args, span);
            }
            ExprKind::NewArray { len } => {
                let elem = match &expr.ty {
                    Ty::Array(elem) => elem.as_ref().clone(),
                    ty => unreachable!("`new` array has the non-array type `{ty}`"),
                };
                let len = self.as_operand(len);
                self.assign(dest, Rvalue::NewArray { elem, len }, span);
            }
            ExprKind::Len { array } => {
                let place = self.as_deref_place(array);
                self.assign(dest, Rvalue::Len(place), span);
            }
            ExprKind::Array { elems } => {
                let elem = match &expr.ty {
                    Ty::Array(elem) => elem.as_ref().clone(),
                    ty => unreachable!("array literal has the non-array type `{ty}`"),
                };
                let operands = elems.iter().map(|elem| self.as_operand(elem)).collect();
                self.assign(
                    dest,
                    Rvalue::Aggregate(Box::new(AggregateKind::Array(elem)), operands),
                    span,
                );
            }
            ExprKind::Unary { op, expr: operand } => match unary_op(*op) {
                Some(op) => {
                    let operand = self.as_operand(operand);
                    self.assign(dest, Rvalue::Unary(op, operand), span);
                }
                None => self.lower_step(Some(dest), *op, operand, span),
            },
            ExprKind::Borrow {
                mutable,
                expr: operand,
            } => {
                let place = self.as_place(operand);
                self.assign(
                    dest,
                    Rvalue::Ref {
                        mutable: *mutable,
                        place,
                    },
                    span,
                );
            }
            ExprKind::Binary {
                op: BinOp::And | BinOp::Or,
                ..
            }
            | ExprKind::Is { .. } => self.bool_into(dest, expr),
            ExprKind::Binary { op, lhs, rhs } => self.lower_binary(dest, *op, lhs, rhs, span),
            ExprKind::Assign { op, place, value } => {
                let place = self.lower_assign(*op, place, value, span);
                let operand = self.operand_from_place(place, &expr.ty, span);
                self.assign(dest, Rvalue::Use(operand), span);
            }
            ExprKind::If { cond, then, els } => {
                let (then_block, else_block) = self.lower_cond(cond);
                let join = self.new_block();
                self.current = then_block;
                self.expr_into(dest.clone(), then);
                self.goto(join, span);
                self.current = else_block;
                self.expr_into(dest, els);
                self.goto(join, span);
                self.current = join;
            }
            ExprKind::Cast { expr: operand } => self.lower_cast(dest, operand, &expr.ty, span),
            ExprKind::Switch { scrutinee, arms } => {
                self.lower_switch(scrutinee, arms, Some(dest), span);
            }
            ExprKind::With { base, fields } => self.lower_with(dest, base, fields, span),
            ExprKind::Equals {
                callee,
                lhs,
                rhs,
                negated,
            } => {
                let args = vec![self.as_operand(lhs), self.as_operand(rhs)];
                if *negated {
                    let equal = self.temp(&Ty::BOOLEAN, span);
                    self.call(
                        callee,
                        Dispatch::Virtual,
                        args,
                        Some(Place::local(equal)),
                        span,
                    );
                    self.assign(
                        dest,
                        Rvalue::Unary(UnaryOp::Not, Operand::Move(Place::local(equal))),
                        span,
                    );
                } else {
                    self.call(callee, Dispatch::Virtual, args, Some(dest), span);
                }
            }
            ExprKind::Closure(closure) => self.lower_closure(dest, closure, &expr.ty, span),
            ExprKind::Unresolved => unreachable!("an unresolved expression survived type checking"),
        }
    }

    pub(super) fn effect(&mut self, expr: &'h Expr) {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Assign { op, place, value } => {
                self.lower_assign(*op, place, value, span);
            }
            ExprKind::Unary { op, expr: operand } if unary_op(*op).is_none() => {
                self.lower_step(None, *op, operand, span);
            }
            ExprKind::Call {
                callee,
                receiver,
                args,
                dispatch,
                safe,
            } => self.lower_call(
                None,
                CallParts {
                    callee,
                    receiver: receiver.as_deref(),
                    args,
                    dispatch: *dispatch,
                    safe: *safe,
                },
                span,
            ),
            ExprKind::CtorCall { callee, this, args } => {
                self.lower_ctor_call(callee, this, args, span);
            }
            ExprKind::Lit(_) | ExprKind::Bool(_) | ExprKind::Null | ExprKind::Local(_) => {}
            _ => {
                let temp = self.temp(&expr.ty, span);
                self.expr_into(Place::local(temp), expr);
            }
        }
    }

    pub(super) fn as_operand(&mut self, expr: &'h Expr) -> Operand {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Lit(lit) => Constant::Lit(*lit).into(),
            ExprKind::Bool(value) => Constant::Bool(*value).into(),
            ExprKind::Null => Constant::Null.into(),
            ExprKind::Static { owner, name } if self.variant_index(*owner, *name).is_some() => {
                self.operand_via_temp(expr)
            }
            ExprKind::Local(_)
            | ExprKind::Static { .. }
            | ExprKind::Field { safe: false, .. }
            | ExprKind::Deref { safe: false, .. }
            | ExprKind::Index { .. } => {
                let place = self.as_place(expr);
                self.operand_from_place(place, &expr.ty, span)
            }
            _ => self.operand_via_temp(expr),
        }
    }

    fn operand_via_temp(&mut self, expr: &'h Expr) -> Operand {
        let temp = self.temp(&expr.ty, expr.span);
        self.expr_into(Place::local(temp), expr);
        Operand::Move(Place::local(temp))
    }

    pub(super) fn operand_from_place(&mut self, place: Place, ty: &Ty, span: Span) -> Operand {
        if let Ty::Ref { mutable: true, .. } = ty {
            let reborrow = self.temp(ty, span);
            self.assign(
                Place::local(reborrow),
                Rvalue::Ref {
                    mutable: true,
                    place: place.project(PlaceElem::Deref),
                },
                span,
            );
            return Operand::Move(Place::local(reborrow));
        }
        if self.is_copy(ty) {
            Operand::Copy(place)
        } else {
            Operand::Move(place)
        }
    }

    pub(super) fn as_place(&mut self, expr: &'h Expr) -> Place {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Local(id) => self.var(*id),
            ExprKind::Static { owner, name } if self.variant_index(*owner, *name).is_none() => {
                Place {
                    base: PlaceBase::Static {
                        owner: *owner,
                        name: *name,
                    },
                    projection: Vec::new(),
                }
            }
            ExprKind::Field {
                base,
                owner,
                name,
                safe: false,
            } => self.as_deref_place(base).project(PlaceElem::Field {
                owner: *owner,
                name: *name,
            }),
            ExprKind::Deref {
                expr: inner,
                safe: false,
            } => self.as_place(inner).project(PlaceElem::Deref),
            ExprKind::Index { base, index } => {
                let array = self.as_deref_place(base);
                let index = self.as_local(index);
                let len = self.temp(&Ty::INT, span);
                self.assign(Place::local(len), Rvalue::Len(array.clone()), span);
                self.check(
                    Check::Bounds {
                        index: Operand::Copy(Place::local(index)),
                        len: Operand::Move(Place::local(len)),
                    },
                    span,
                );
                array.project(PlaceElem::Index(index))
            }
            _ => {
                let temp = self.temp(&expr.ty, span);
                self.expr_into(Place::local(temp), expr);
                Place::local(temp)
            }
        }
    }

    pub(super) fn as_deref_place(&mut self, expr: &'h Expr) -> Place {
        let place = self.as_place(expr);
        deref(place, &expr.ty)
    }

    fn as_local(&mut self, expr: &'h Expr) -> crate::mir::Local {
        if let ExprKind::Local(id) = &expr.kind
            && let Some(local) = self.var(*id).as_local()
        {
            return local;
        }
        let operand = self.as_operand(expr);
        let temp = self.temp(&expr.ty, expr.span);
        self.assign(Place::local(temp), Rvalue::Use(operand), expr.span);
        temp
    }

    pub(super) fn check(&mut self, check: Check, span: Span) {
        let target = self.new_block();
        let unwind = self.unwind();
        self.terminate(
            TerminatorKind::Assert {
                check,
                target,
                unwind,
            },
            span,
        );
        self.current = target;
    }

    pub(super) fn branch(&mut self, cond: Operand, span: Span) -> (BasicBlock, BasicBlock) {
        let then_block = self.new_block();
        let else_block = self.new_block();
        self.terminate(
            TerminatorKind::SwitchInt {
                discr: cond,
                targets: vec![(0, else_block)],
                otherwise: then_block,
            },
            span,
        );
        (then_block, else_block)
    }

    fn merge(&mut self, a: BasicBlock, b: BasicBlock, span: Span) -> BasicBlock {
        let join = self.new_block();
        for block in [a, b] {
            self.current = block;
            self.goto(join, span);
        }
        join
    }

    pub(super) fn null_branch(&mut self, place: Place, span: Span) -> (BasicBlock, BasicBlock) {
        let is_null = self.temp(&Ty::BOOLEAN, span);
        self.assign(Place::local(is_null), Rvalue::IsNull(place), span);
        self.branch(Operand::Move(Place::local(is_null)), span)
    }

    pub(super) fn lower_cond(&mut self, expr: &'h Expr) -> (BasicBlock, BasicBlock) {
        let span = expr.span;
        match &expr.kind {
            ExprKind::Binary {
                op: BinOp::And,
                lhs,
                rhs,
            } => {
                let (lhs_true, lhs_false) = self.lower_cond(lhs);
                self.current = lhs_true;
                let (rhs_true, rhs_false) = self.lower_cond(rhs);
                (rhs_true, self.merge(lhs_false, rhs_false, span))
            }
            ExprKind::Binary {
                op: BinOp::Or,
                lhs,
                rhs,
            } => {
                let (lhs_true, lhs_false) = self.lower_cond(lhs);
                self.current = lhs_false;
                let (rhs_true, rhs_false) = self.lower_cond(rhs);
                (self.merge(lhs_true, rhs_true, span), rhs_false)
            }
            ExprKind::Unary {
                op: UnOp::Not,
                expr: operand,
            } => {
                let (then_block, else_block) = self.lower_cond(operand);
                (else_block, then_block)
            }
            ExprKind::Is {
                expr: scrutinee,
                pat,
            } => {
                let place = self.as_place(scrutinee);
                let fail = self.new_block();
                let mut bindings = Vec::new();
                self.test_pat(place, &scrutinee.ty, pat, fail, &mut bindings);
                self.bind(bindings, span);
                (self.current, fail)
            }
            _ => {
                let cond = self.as_operand(expr);
                self.branch(cond, span)
            }
        }
    }

    fn bool_into(&mut self, dest: Place, expr: &'h Expr) {
        let span = expr.span;
        let (then_block, else_block) = self.lower_cond(expr);
        let join = self.new_block();
        for (block, value) in [(then_block, true), (else_block, false)] {
            self.current = block;
            self.assign(
                dest.clone(),
                Rvalue::Use(Constant::Bool(value).into()),
                span,
            );
            self.goto(join, span);
        }
        self.current = join;
    }

    fn lower_binary(&mut self, dest: Place, op: BinOp, lhs: &'h Expr, rhs: &'h Expr, span: Span) {
        match op {
            BinOp::Elvis => {
                let place = self.as_place(lhs);
                let (null, present) = self.null_branch(place.clone(), span);
                let join = self.new_block();
                self.current = null;
                self.expr_into(dest.clone(), rhs);
                self.goto(join, span);
                self.current = present;
                let operand = self.operand_from_place(place, lhs.ty.non_null(), span);
                self.assign(dest, Rvalue::Use(operand), span);
                self.goto(join, span);
                self.current = join;
            }
            BinOp::Eq | BinOp::Ne if is_null(lhs) || is_null(rhs) => {
                let tested = if is_null(lhs) { rhs } else { lhs };
                let place = self.as_place(tested);
                if op == BinOp::Eq {
                    self.assign(dest, Rvalue::IsNull(place), span);
                } else {
                    let is_null = self.temp(&Ty::BOOLEAN, span);
                    self.assign(Place::local(is_null), Rvalue::IsNull(place), span);
                    self.assign(
                        dest,
                        Rvalue::Unary(UnaryOp::Not, Operand::Move(Place::local(is_null))),
                        span,
                    );
                }
            }
            _ => {
                let lhs_operand = self.as_operand(lhs);
                let rhs_operand = self.as_operand(rhs);
                self.check_divisor(op, &lhs.ty, &rhs_operand, span);
                self.assign(dest, Rvalue::Binary(op, lhs_operand, rhs_operand), span);
            }
        }
    }

    fn check_divisor(&mut self, op: BinOp, ty: &Ty, divisor: &Operand, span: Span) {
        let integral = ty.primitive().is_some_and(krelpin_typeck::ty::is_integral);
        if !matches!(op, BinOp::Div | BinOp::Rem) || !integral {
            return;
        }
        if let Operand::Constant(Constant::Lit(lit)) = divisor
            && !is_zero_literal(lit)
        {
            return;
        }
        self.check(Check::NonZero(divisor.clone()), span);
    }

    pub(super) fn lower_assign(
        &mut self,
        op: Option<BinOp>,
        place: &'h Expr,
        value: &'h Expr,
        span: Span,
    ) -> Place {
        let target = self.as_place(place);
        match op {
            None => {
                let operand = self.as_operand(value);
                self.assign(target.clone(), Rvalue::Use(operand), span);
            }
            Some(op) => {
                let current = self.operand_from_place(target.clone(), &place.ty, span);
                let operand = self.as_operand(value);
                self.check_divisor(op, &place.ty, &operand, span);
                self.assign(target.clone(), Rvalue::Binary(op, current, operand), span);
            }
        }
        target
    }

    fn lower_step(&mut self, dest: Option<Place>, op: UnOp, operand: &'h Expr, span: Span) {
        let place = self.as_place(operand);
        let primitive = operand
            .ty
            .primitive()
            .unwrap_or_else(|| unreachable!("`{}` cannot be incremented", operand.ty));
        let one = Constant::Int {
            value: 1,
            ty: primitive,
        };
        let (bin_op, post) = match op {
            UnOp::PreInc => (BinOp::Add, false),
            UnOp::PreDec => (BinOp::Sub, false),
            UnOp::PostInc => (BinOp::Add, true),
            UnOp::PostDec => (BinOp::Sub, true),
            UnOp::Neg | UnOp::Not | UnOp::BitNot => unreachable!("`{op:?}` is not a step"),
        };
        if let (true, Some(dest)) = (post, &dest) {
            self.assign(
                dest.clone(),
                Rvalue::Use(Operand::Copy(place.clone())),
                span,
            );
        }
        self.assign(
            place.clone(),
            Rvalue::Binary(bin_op, Operand::Copy(place.clone()), one.into()),
            span,
        );
        if let (false, Some(dest)) = (post, dest) {
            self.assign(dest, Rvalue::Use(Operand::Copy(place)), span);
        }
    }

    fn lower_cast(&mut self, dest: Place, operand: &'h Expr, target: &Ty, span: Span) {
        if self.needs_cast_check(&operand.ty, target) {
            let place = self.as_place(operand);
            self.check(
                Check::InstanceOf {
                    place: place.clone(),
                    ty: target.non_null().clone(),
                },
                span,
            );
            let value = self.operand_from_place(place, &operand.ty, span);
            self.assign(dest, Rvalue::Cast(value, target.clone()), span);
            return;
        }
        let value = self.as_operand(operand);
        self.assign(dest, Rvalue::Cast(value, target.clone()), span);
    }

    fn needs_cast_check(&self, from: &Ty, to: &Ty) -> bool {
        let checked = matches!(to.non_null(), Ty::Adt { .. } | Ty::Array(_));
        let reference = matches!(
            from.non_null(),
            Ty::Adt { .. } | Ty::Array(_) | Ty::Param(_) | Ty::External { .. }
        );
        checked && reference && !self.statically_is(from, to)
    }

    pub(super) fn statically_is(&self, from: &Ty, to: &Ty) -> bool {
        let from = from.strip_refs().non_null();
        let to = to.strip_refs().non_null();
        if from == to {
            return true;
        }
        match (from, to) {
            (_, Ty::Adt { def, .. }) if self.tcx().lang_items.is(*def, LangItem::Object) => {
                !matches!(from, Ty::Primitive(_))
            }
            (Ty::Adt { def: sub, .. }, Ty::Adt { def: sup, .. }) => {
                self.tcx().is_subclass(*sub, *sup)
            }
            _ => false,
        }
    }

    pub(super) fn lower_call(&mut self, dest: Option<Place>, parts: CallParts<'h>, span: Span) {
        let CallParts {
            callee,
            receiver,
            args,
            dispatch,
            safe,
        } = parts;
        if receiver.is_none()
            && let Some(index) = self.variant_index(callee.method.owner, callee.method.name.name)
        {
            let operands = args.iter().map(|arg| self.as_operand(arg)).collect();
            let dest = dest.unwrap_or_else(|| Place::local(self.temp(&callee.ret, span)));
            self.assign(
                dest,
                Rvalue::Aggregate(
                    Box::new(AggregateKind::Variant {
                        def: callee.method.owner,
                        index,
                        ty: callee.ret.clone(),
                    }),
                    operands,
                ),
                span,
            );
            return;
        }

        let Some(receiver) = receiver.filter(|_| safe) else {
            let wants_mut = callee.method.receiver == Some(ReceiverKind::RefMut);
            let staged = receiver.map(|receiver| self.stage_receiver(receiver, wants_mut));
            let deferred = matches!(staged, Some(StagedReceiver::Place { .. }));
            let args: Vec<Operand> = args
                .iter()
                .map(|arg| {
                    let operand = self.as_operand(arg);
                    match operand {
                        Operand::Copy(_) | Operand::Move(_) if deferred => {
                            let temp = self.temp(&arg.ty, arg.span);
                            self.assign(Place::local(temp), Rvalue::Use(operand), arg.span);
                            Operand::Move(Place::local(temp))
                        }
                        operand => operand,
                    }
                })
                .collect();
            let mut operands: Vec<Operand> = staged
                .map(|staged| self.activate_receiver(staged, span))
                .into_iter()
                .collect();
            operands.extend(args);
            self.call(callee, dispatch, operands, dest, span);
            return;
        };

        let (inner, borrow) = match &receiver.kind {
            ExprKind::Borrow { mutable, expr } => (expr.as_ref(), Some(*mutable)),
            _ => (receiver, None),
        };
        let base = self.as_place(inner);
        let (null, present) = self.null_branch(base.clone(), span);
        let join = self.new_block();
        self.current = null;
        if let Some(dest) = &dest {
            self.assign(dest.clone(), Rvalue::Use(Constant::Null.into()), span);
        }
        self.goto(join, span);
        self.current = present;
        let receiver = match borrow {
            Some(mutable) => {
                let ty = Ty::Ref {
                    mutable,
                    ty: Box::new(inner.ty.non_null().clone()),
                };
                let temp = self.temp(&ty, span);
                self.assign(
                    Place::local(temp),
                    Rvalue::Ref {
                        mutable,
                        place: base,
                    },
                    span,
                );
                Operand::Move(Place::local(temp))
            }
            None => self.operand_from_place(base, inner.ty.non_null(), span),
        };
        let mut operands = vec![receiver];
        operands.extend(args.iter().map(|arg| self.as_operand(arg)));
        self.call(callee, dispatch, operands, dest, span);
        self.goto(join, span);
        self.current = join;
    }

    fn stage_receiver(&mut self, receiver: &'h Expr, wants_mut: bool) -> StagedReceiver {
        if let (true, Ty::Ref { mutable: false, ty }) = (wants_mut, &receiver.ty) {
            return StagedReceiver::Place {
                place: self.as_place(receiver).project(PlaceElem::Deref),
                ty: ty.as_ref().clone(),
                borrow: Some(true),
            };
        }
        match &receiver.kind {
            ExprKind::Borrow { mutable, expr } if self.is_place_expr(expr) => {
                StagedReceiver::Place {
                    place: self.as_place(expr),
                    ty: expr.ty.clone(),
                    borrow: Some(*mutable),
                }
            }
            _ if self.is_place_expr(receiver) => StagedReceiver::Place {
                place: self.as_place(receiver),
                ty: receiver.ty.clone(),
                borrow: None,
            },
            _ => StagedReceiver::Ready(self.as_operand(receiver)),
        }
    }

    fn activate_receiver(&mut self, staged: StagedReceiver, span: Span) -> Operand {
        match staged {
            StagedReceiver::Ready(operand) => operand,
            StagedReceiver::Place {
                place,
                ty,
                borrow: None,
            } => self.operand_from_place(place, &ty, span),
            StagedReceiver::Place {
                place,
                ty,
                borrow: Some(mutable),
            } => {
                let ref_ty = Ty::Ref {
                    mutable,
                    ty: Box::new(ty),
                };
                let temp = self.temp(&ref_ty, span);
                self.assign(Place::local(temp), Rvalue::Ref { mutable, place }, span);
                Operand::Move(Place::local(temp))
            }
        }
    }

    fn is_place_expr(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Local(_)
            | ExprKind::Field { safe: false, .. }
            | ExprKind::Deref { safe: false, .. }
            | ExprKind::Index { .. } => true,
            ExprKind::Static { owner, name } => self.variant_index(*owner, *name).is_none(),
            _ => false,
        }
    }

    pub(super) fn call(
        &mut self,
        callee: &Callee,
        dispatch: Dispatch,
        args: Vec<Operand>,
        dest: Option<Place>,
        span: Span,
    ) {
        let dest = match (&callee.ret, dest) {
            (Ty::Void, _) => None,
            (_, Some(dest)) => Some(dest),
            (ret, None) => Some(Place::local(self.temp(ret, span))),
        };
        let target = self.new_block();
        let unwind = self.unwind();
        self.terminate(
            TerminatorKind::Call {
                callee: Box::new(callee.clone()),
                dispatch,
                args,
                dest,
                target,
                unwind,
            },
            span,
        );
        self.current = target;
    }

    pub(super) fn receiver_operand(
        &mut self,
        place: Place,
        ty: &Ty,
        kind: Option<ReceiverKind>,
        span: Span,
    ) -> Operand {
        let mutable = match (kind, ty) {
            (Some(ReceiverKind::Owned) | None, _) | (_, Ty::Ref { .. }) => {
                return self.operand_from_place(place, ty, span);
            }
            (Some(ReceiverKind::Ref), _) => false,
            (Some(ReceiverKind::RefMut), _) => true,
        };
        let ref_ty = Ty::Ref {
            mutable,
            ty: Box::new(ty.clone()),
        };
        let temp = self.temp(&ref_ty, span);
        self.assign(Place::local(temp), Rvalue::Ref { mutable, place }, span);
        Operand::Move(Place::local(temp))
    }

    fn lower_new(
        &mut self,
        dest: Place,
        ty: &Ty,
        callee: &'h Option<Box<Callee>>,
        args: &'h [Expr],
        span: Span,
    ) {
        let Ty::Adt { def, .. } = ty else {
            unreachable!("`new` builds the non-class type `{ty}`");
        };
        let is_record = self.tcx().sig(*def).kind == AdtKind::Record;
        if is_record && !self.cx.constructed.contains(def) {
            let operands = args.iter().map(|arg| self.as_operand(arg)).collect();
            self.assign(
                dest,
                Rvalue::Aggregate(
                    Box::new(AggregateKind::Record {
                        def: *def,
                        ty: ty.clone(),
                    }),
                    operands,
                ),
                span,
            );
            return;
        }

        let object = self.temp(ty, span);
        self.assign(Place::local(object), Rvalue::New(ty.clone()), span);
        if let Some(callee) = callee {
            let this =
                self.receiver_operand(Place::local(object), ty, Some(ReceiverKind::RefMut), span);
            let mut operands = vec![this];
            operands.extend(args.iter().map(|arg| self.as_operand(arg)));
            self.call(callee, Dispatch::Static, operands, None, span);
        }
        self.assign(dest, Rvalue::Use(Operand::Move(Place::local(object))), span);
    }

    fn lower_ctor_call(&mut self, callee: &Callee, this: &'h Expr, args: &'h [Expr], span: Span) {
        let mut operands = vec![self.as_operand(this)];
        operands.extend(args.iter().map(|arg| self.as_operand(arg)));
        self.call(callee, Dispatch::Static, operands, None, span);
    }

    fn lower_with(&mut self, dest: Place, base: &'h Expr, fields: &'h [WithField], span: Span) {
        let record = base.ty.strip_refs().clone();
        let Ty::Adt { def, args, .. } = &record else {
            unreachable!("`with` on the non-record type `{record}`");
        };
        let place = self.as_deref_place(base);
        let mut values: HashMap<_, Operand> = HashMap::new();
        for field in fields {
            let operand = self.as_operand(&field.value);
            values.insert(field.name, operand);
        }
        let tcx = self.tcx();
        let sig = tcx.sig(*def);
        let subst = tcx.subst_for(*def, args);
        let mut operands = Vec::with_capacity(sig.components.len());
        for name in &sig.components {
            let operand = match values.remove(name) {
                Some(operand) => operand,
                None => {
                    let ty = sig
                        .fields
                        .get(name)
                        .map_or(Ty::Unknown, |field| field.ty.subst(&subst));
                    let component = place.clone().project(PlaceElem::Field {
                        owner: *def,
                        name: *name,
                    });
                    self.operand_from_place(component, &ty, span)
                }
            };
            operands.push(operand);
        }
        self.assign(
            dest,
            Rvalue::Aggregate(
                Box::new(AggregateKind::Record {
                    def: *def,
                    ty: record.clone(),
                }),
                operands,
            ),
            span,
        );
    }

    fn lower_closure(&mut self, dest: Place, closure: &'h Closure, ty: &Ty, span: Span) {
        let (target, operands) = match &closure.kind {
            ClosureKind::Lambda {
                moves,
                params,
                ret,
                body,
            } => {
                let captures: Vec<CaptureInfo> = closure
                    .captures
                    .iter()
                    .map(|capture| {
                        let decl = self.hir.local(capture.local);
                        let mode = if *moves || (!capture.mutable && self.is_copy(&decl.ty)) {
                            CaptureMode::Value
                        } else if capture.mutable {
                            CaptureMode::RefMut
                        } else {
                            CaptureMode::Ref
                        };
                        CaptureInfo {
                            name: decl.name,
                            ty: decl.ty.clone(),
                            mode,
                        }
                    })
                    .collect();
                let operands = closure
                    .captures
                    .iter()
                    .zip(&captures)
                    .map(|(capture, info)| {
                        let place = self.var(capture.local);
                        let mutable = match info.mode {
                            CaptureMode::Value => {
                                return self.operand_from_place(place, &info.ty, span);
                            }
                            CaptureMode::Ref => false,
                            CaptureMode::RefMut => true,
                        };
                        let ref_ty = Ty::Ref {
                            mutable,
                            ty: Box::new(info.ty.clone()),
                        };
                        let temp = self.temp(&ref_ty, span);
                        self.assign(Place::local(temp), Rvalue::Ref { mutable, place }, span);
                        Operand::Move(Place::local(temp))
                    })
                    .collect();
                let id = self.build_closure(closure, captures, params, ret, body, ty, span);
                (ClosureTarget::Lambda(id), operands)
            }
            ClosureKind::MethodRef {
                callee,
                bound,
                constructs,
            } => {
                let operands = bound
                    .as_deref()
                    .map(|bound| self.as_operand(bound))
                    .into_iter()
                    .collect();
                (
                    ClosureTarget::MethodRef {
                        callee: callee.clone(),
                        constructs: *constructs,
                    },
                    operands,
                )
            }
        };
        self.assign(
            dest,
            Rvalue::Aggregate(Box::new(AggregateKind::Closure(target)), operands),
            span,
        );
    }

    pub(super) fn variant_index(&self, owner: DefId, name: krelpin_span::Symbol) -> Option<usize> {
        let sig = self.tcx().sig(owner);
        if sig.kind != AdtKind::Enum {
            return None;
        }
        sig.variants
            .iter()
            .position(|variant| variant.name.name == name)
    }
}

enum StagedReceiver {
    Ready(Operand),
    Place {
        place: Place,
        ty: Ty,
        borrow: Option<bool>,
    },
}

pub(super) fn deref(mut place: Place, ty: &Ty) -> Place {
    let mut ty = ty;
    while let Ty::Ref { ty: inner, .. } = ty {
        place = place.project(PlaceElem::Deref);
        ty = inner;
    }
    place
}

fn unary_op(op: UnOp) -> Option<UnaryOp> {
    match op {
        UnOp::Neg => Some(UnaryOp::Neg),
        UnOp::Not => Some(UnaryOp::Not),
        UnOp::BitNot => Some(UnaryOp::BitNot),
        UnOp::PreInc | UnOp::PreDec | UnOp::PostInc | UnOp::PostDec => None,
    }
}

fn is_null(expr: &Expr) -> bool {
    matches!(expr.kind, ExprKind::Null)
}

fn is_zero_literal(lit: &Lit) -> bool {
    if lit.kind != LitKind::Integer {
        return false;
    }
    let text = lit.symbol.as_str().to_ascii_lowercase();
    let digits = ["0x", "0b", "0o"]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .unwrap_or(&text);
    digits
        .chars()
        .filter(|c| *c != '_')
        .all(|c| c == '0' || c == 'l')
}

impl From<Constant> for Operand {
    fn from(constant: Constant) -> Operand {
        Operand::Constant(constant)
    }
}

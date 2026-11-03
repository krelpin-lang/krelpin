use krelpin_ast::ast::{BinOp, PrimitiveTy};
use krelpin_hir::{Block, Catch, Dispatch, Expr, LocalId, LoopKind, Stmt, StmtKind};
use krelpin_span::{Span, Symbol};
use krelpin_typeck::{Callee, LangItem, MethodSig, Ty};

use super::{Builder, Scope};
use crate::mir::{BasicBlock, Constant, Local, Operand, Place, PlaceElem, Rvalue, TerminatorKind};

struct IterMethods {
    iterator: Callee,
    has_next: Callee,
    next: Callee,
}

impl<'h> Builder<'_, 'h, '_> {
    pub(super) fn lower_block(&mut self, block: &'h Block) {
        self.push_block_scope();
        for stmt in &block.stmts {
            self.lower_stmt(stmt);
        }
        self.pop_block_scope(block.span.shrink_to_hi());
    }

    fn lower_stmt(&mut self, stmt: &'h Stmt) {
        let span = stmt.span;
        match &stmt.kind {
            StmtKind::Let { local, init } => {
                let local = self.declare_var(*local);
                if let Some(init) = init {
                    self.expr_into(Place::local(local), init);
                }
            }
            StmtKind::Expr(expr) => self.effect(expr),
            StmtKind::Block(block) => self.lower_block(block),
            StmtKind::If { cond, then, els } => {
                let (then_block, else_block) = self.lower_cond(cond);
                let join = self.new_block();
                self.current = then_block;
                self.lower_block(then);
                self.goto(join, span);
                self.current = else_block;
                if let Some(els) = els {
                    self.lower_block(els);
                }
                self.goto(join, span);
                self.current = join;
            }
            StmtKind::Loop {
                kind,
                cond,
                update,
                body,
            } => self.lower_loop(*kind, cond.as_ref(), update, body, span),
            StmtKind::ForEach { local, iter, body } => {
                self.lower_for_each(*local, iter, body, span)
            }
            StmtKind::Switch { scrutinee, arms } => self.lower_switch(scrutinee, arms, None, span),
            StmtKind::Try {
                body,
                catches,
                finally,
            } => self.lower_try(body, catches, finally.as_ref(), span),
            StmtKind::Return(value) => {
                if let Some(value) = value {
                    self.expr_into(Place::local(Local::RETURN), value);
                }
                self.exit_scopes(0, span);
                self.diverge(TerminatorKind::Return, span);
            }
            StmtKind::Yield(value) => {
                let (depth, dest, exit) = self
                    .scopes
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(depth, scope)| match scope {
                        Scope::Value { dest, exit } => Some((depth, dest.clone(), *exit)),
                        _ => None,
                    })
                    .unwrap_or_else(|| unreachable!("`yield` outside a `switch` expression"));
                self.expr_into(dest, value);
                self.exit_scopes(depth + 1, span);
                self.diverge(TerminatorKind::Goto { target: exit }, span);
            }
            StmtKind::Throw(value) => {
                let value = self.as_operand(value);
                let unwind = self.unwind();
                self.diverge(TerminatorKind::Throw { value, unwind }, span);
            }
            StmtKind::Break => {
                let (depth, target) = self
                    .scopes
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(depth, scope)| match scope {
                        Scope::Loop { exit, .. } | Scope::Breakable { exit } => {
                            Some((depth, *exit))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| unreachable!("`break` outside a loop or `switch`"));
                self.exit_scopes(depth + 1, span);
                self.diverge(TerminatorKind::Goto { target }, span);
            }
            StmtKind::Continue => {
                let (depth, target) = self
                    .scopes
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(depth, scope)| match scope {
                        Scope::Loop { next, .. } => Some((depth, *next)),
                        _ => None,
                    })
                    .unwrap_or_else(|| unreachable!("`continue` outside a loop"));
                self.exit_scopes(depth + 1, span);
                self.diverge(TerminatorKind::Goto { target }, span);
            }
        }
    }

    fn lower_loop(
        &mut self,
        kind: LoopKind,
        cond: Option<&'h Expr>,
        update: &'h [Expr],
        body: &'h Block,
        span: Span,
    ) {
        match kind {
            LoopKind::While => {
                let header = self.goto_new(span);
                let (body_block, exit) = match cond {
                    Some(cond) => self.lower_cond(cond),
                    None => (self.current, self.new_block()),
                };
                let next = self.new_block();
                self.current = body_block;
                self.scopes.push(Scope::Loop { exit, next });
                self.lower_block(body);
                self.scopes.pop();
                self.goto(next, span);
                self.current = next;
                for expr in update {
                    self.effect(expr);
                }
                self.goto(header, span);
                self.current = exit;
            }
            LoopKind::DoWhile => {
                let body_block = self.goto_new(span);
                let next = self.new_block();
                let exit = self.new_block();
                self.scopes.push(Scope::Loop { exit, next });
                self.lower_block(body);
                self.scopes.pop();
                self.goto(next, span);
                self.current = next;
                match cond {
                    Some(cond) => {
                        let (again, done) = self.lower_cond(cond);
                        self.current = again;
                        self.goto(body_block, span);
                        self.current = done;
                        self.goto(exit, span);
                    }
                    None => self.goto(body_block, span),
                }
                self.current = exit;
            }
        }
    }

    fn lower_for_each(&mut self, local: LocalId, iter: &'h Expr, body: &'h Block, span: Span) {
        match iter.ty.strip_refs() {
            Ty::Array(elem) => {
                let elem = elem.as_ref().clone();
                self.lower_array_loop(local, iter, &elem, body, span);
            }
            _ => self.lower_iterable_loop(local, iter, body, span),
        }
    }

    fn lower_array_loop(
        &mut self,
        local: LocalId,
        iter: &'h Expr,
        elem: &Ty,
        body: &'h Block,
        span: Span,
    ) {
        let array = self.as_deref_place(iter);
        let index = self.temp(&Ty::INT, span);
        self.assign(Place::local(index), Rvalue::Use(int(0).into()), span);
        let len = self.temp(&Ty::INT, span);
        self.assign(Place::local(len), Rvalue::Len(array.clone()), span);

        let header = self.goto_new(span);
        let in_bounds = self.temp(&Ty::BOOLEAN, span);
        self.assign(
            Place::local(in_bounds),
            Rvalue::Binary(
                BinOp::Lt,
                Operand::Copy(Place::local(index)),
                Operand::Copy(Place::local(len)),
            ),
            span,
        );
        let (body_block, exit) = self.branch(Operand::Move(Place::local(in_bounds)), span);
        let next = self.new_block();

        self.current = body_block;
        self.scopes.push(Scope::Loop { exit, next });
        self.push_block_scope();
        let element = array.project(PlaceElem::Index(index));
        self.bind_element(local, element, elem, span);
        self.lower_block(body);
        self.pop_block_scope(span);
        self.scopes.pop();
        self.goto(next, span);

        self.current = next;
        self.assign(
            Place::local(index),
            Rvalue::Binary(
                BinOp::Add,
                Operand::Copy(Place::local(index)),
                Constant::Int {
                    value: 1,
                    ty: PrimitiveTy::Int,
                }
                .into(),
            ),
            span,
        );
        self.goto(header, span);
        self.current = exit;
    }

    fn lower_iterable_loop(&mut self, local: LocalId, iter: &'h Expr, body: &'h Block, span: Span) {
        let methods = self.iter_methods(&iter.ty);
        let iterable = self.as_place(iter);
        let receiver =
            self.receiver_operand(iterable, &iter.ty, methods.iterator.method.receiver, span);
        let iterator = self.temp(&methods.iterator.ret, span);
        self.call(
            &methods.iterator,
            Dispatch::Virtual,
            vec![receiver],
            Some(Place::local(iterator)),
            span,
        );

        let header = self.goto_new(span);
        let has_next = self.temp(&Ty::BOOLEAN, span);
        let receiver = self.receiver_operand(
            Place::local(iterator),
            &methods.iterator.ret,
            methods.has_next.method.receiver,
            span,
        );
        self.call(
            &methods.has_next,
            Dispatch::Virtual,
            vec![receiver],
            Some(Place::local(has_next)),
            span,
        );
        let (body_block, exit) = self.branch(Operand::Move(Place::local(has_next)), span);

        self.current = body_block;
        self.scopes.push(Scope::Loop { exit, next: header });
        self.push_block_scope();
        let element = self.temp(&methods.next.ret, span);
        let receiver = self.receiver_operand(
            Place::local(iterator),
            &methods.iterator.ret,
            methods.next.method.receiver,
            span,
        );
        self.call(
            &methods.next,
            Dispatch::Virtual,
            vec![receiver],
            Some(Place::local(element)),
            span,
        );
        let elem_ty = methods.next.ret.clone();
        self.bind_element(local, Place::local(element), &elem_ty, span);
        self.lower_block(body);
        self.pop_block_scope(span);
        self.scopes.pop();
        self.goto(header, span);
        self.current = exit;
    }

    fn bind_element(&mut self, local: LocalId, element: Place, elem: &Ty, span: Span) {
        let local = self.declare_var(local);
        let rvalue = match self.locals[local.index()].ty.clone() {
            Ty::Ref { mutable, .. } if !matches!(elem, Ty::Ref { .. }) => Rvalue::Ref {
                mutable,
                place: element,
            },
            _ => Rvalue::Use(self.operand_from_place(element, elem, span)),
        };
        self.assign(Place::local(local), rvalue, span);
    }

    fn iter_methods(&self, ty: &Ty) -> IterMethods {
        let tcx = self.tcx();
        let Ty::Adt { def, args, .. } = ty.strip_refs() else {
            unreachable!("`for` loops over the non-iterable type `{ty}`");
        };
        let iterable = tcx
            .lang_items
            .get(LangItem::Iterable)
            .unwrap_or_else(|| unreachable!("the standard library defines `Iterable`"));
        let iterable_args = tcx
            .upcast(*def, args, iterable)
            .unwrap_or_else(|| unreachable!("`{ty}` is checked to be `Iterable`"));
        let iterator = self.method(iterable, "iterator");
        let iterator_ret = iterator.ret.subst(&tcx.subst_for(iterable, &iterable_args));
        let Ty::Adt {
            def: iterator_def,
            args: iterator_args,
            ..
        } = &iterator_ret
        else {
            unreachable!("`Iterable.iterator` returns an iterator");
        };
        let subst = tcx.subst_for(*iterator_def, iterator_args);
        let has_next = self.method(*iterator_def, "hasNext");
        let next = self.method(*iterator_def, "next");
        let next_ret = next.ret.subst(&subst);
        IterMethods {
            iterator: callee(iterator, iterator_ret.clone()),
            has_next: callee(has_next, Ty::BOOLEAN),
            next: callee(next, next_ret),
        }
    }

    fn method(&self, owner: krelpin_span::def_id::DefId, name: &str) -> MethodSig {
        self.tcx()
            .sig(owner)
            .methods
            .get(&Symbol::intern(name))
            .and_then(|methods| methods.first())
            .cloned()
            .unwrap_or_else(|| unreachable!("the standard library declares `{name}`"))
    }

    fn lower_try(
        &mut self,
        body: &'h Block,
        catches: &'h [Catch],
        finally: Option<&'h Block>,
        span: Span,
    ) {
        let join = self.new_block();
        let handler = self.new_block();

        self.scopes.push(Scope::Try {
            finally,
            handler: Some(handler),
        });
        self.lower_block(body);
        self.scopes.pop();
        self.complete_try(finally, join, span);

        self.current = handler;
        let throwable = self.tcx().lang_ty(LangItem::Throwable);
        let exception = self.temp(&throwable, span);
        self.assign(Place::local(exception), Rvalue::Caught, span);

        let catch_handler = finally.map(|_| self.new_block());
        for catch in catches {
            let caught = self.new_block();
            for ty in &catch.tys {
                let is = self.temp(&Ty::BOOLEAN, span);
                self.assign(
                    Place::local(is),
                    Rvalue::InstanceOf(Place::local(exception), ty.clone()),
                    span,
                );
                let (matched, rest) = self.branch(Operand::Move(Place::local(is)), span);
                self.current = matched;
                self.goto(caught, span);
                self.current = rest;
            }
            let rest = self.current;

            self.current = caught;
            self.scopes.push(Scope::Try {
                finally,
                handler: catch_handler,
            });
            self.push_block_scope();
            let local = self.declare_var(catch.local);
            let ty = self.locals[local.index()].ty.clone();
            self.assign(
                Place::local(local),
                Rvalue::Use(Operand::Move(
                    Place::local(exception).project(PlaceElem::Subtype(ty)),
                )),
                span,
            );
            self.lower_block(&catch.body);
            self.pop_block_scope(span);
            self.scopes.pop();
            self.complete_try(finally, join, span);
            self.current = rest;
        }
        self.rethrow(exception, finally, span);

        if let Some(catch_handler) = catch_handler {
            self.current = catch_handler;
            let exception = self.temp(&throwable, span);
            self.assign(Place::local(exception), Rvalue::Caught, span);
            self.rethrow(exception, finally, span);
        }
        self.current = join;
    }

    fn complete_try(&mut self, finally: Option<&'h Block>, join: BasicBlock, span: Span) {
        if let Some(finally) = finally {
            self.lower_block(finally);
        }
        self.goto(join, span);
    }

    fn rethrow(&mut self, exception: Local, finally: Option<&'h Block>, span: Span) {
        if let Some(finally) = finally {
            self.lower_block(finally);
        }
        let unwind = self.unwind();
        self.diverge(
            TerminatorKind::Throw {
                value: Operand::Move(Place::local(exception)),
                unwind,
            },
            span,
        );
    }
}

fn callee(method: MethodSig, ret: Ty) -> Callee {
    Callee {
        params: Vec::new(),
        ret,
        method,
        packs_varargs: false,
    }
}

fn int(value: i64) -> Constant {
    Constant::Int {
        value,
        ty: PrimitiveTy::Int,
    }
}

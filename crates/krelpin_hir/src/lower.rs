use std::collections::HashMap;

use krelpin_ast::ast::{
    self, CaseLabel, CompilationUnit, FieldStorage, FnRetTy, ItemKind, Member, MemberKind, Param,
    ReceiverKind, SwitchExprBody,
};
use krelpin_resolve::{Res, ResolverOutput};
use krelpin_span::def_id::DefId;
use krelpin_span::{Span, Symbol, kw};
use krelpin_typeck::{Callee, PatRes, Tcx, Ty, TypeckResults};

use crate::hir::{
    Arm, ArmBody, Block, Body, BodyKind, BodyValue, Catch, Dispatch, Expr, ExprKind, Hir,
    LocalDecl, LocalId, LoopKind, Pat, PatKind, Stmt, StmtKind,
};

pub fn lower(
    units: &[CompilationUnit],
    resolved: &ResolverOutput,
    tcx: &Tcx<'_>,
    results: &TypeckResults,
) -> Hir {
    let mut hir = Hir::default();
    for (type_def, item) in resolved.items.items(units) {
        let members: &[Member] = match &item.kind {
            ItemKind::Class(class) => &class.members,
            ItemKind::Struct(def) => &def.members,
            ItemKind::Interface(interface) => &interface.members,
            ItemKind::Enum(def) => &def.members,
        };
        for member in members {
            let cx = LoweringCtxt {
                tcx,
                resolved,
                results,
                owner: type_def.def,
                locals: Vec::new(),
                by_decl: HashMap::new(),
                this: None,
            };
            if let Some(body) = cx.lower_member(member) {
                hir.bodies.push(body);
            }
        }
    }
    hir
}

struct LoweringCtxt<'a, 'tcx> {
    tcx: &'a Tcx<'tcx>,
    resolved: &'a ResolverOutput,
    results: &'a TypeckResults,
    owner: DefId,
    locals: Vec<LocalDecl>,
    by_decl: HashMap<Span, LocalId>,
    this: Option<LocalId>,
}

impl LoweringCtxt<'_, '_> {
    fn lower_member(mut self, member: &Member) -> Option<Body> {
        let (name, kind, receiver, params, ret): (_, _, _, &[Param], _) = match &member.kind {
            MemberKind::Type(_) => return None,
            MemberKind::Field(field) => {
                field.init.as_ref()?;
                let receiver =
                    (field.storage == FieldStorage::Instance).then_some(ReceiverKind::RefMut);
                let ret = self.tcx.lower_ty(&field.ty);
                (field.name.name, BodyKind::FieldInit, receiver, &[], ret)
            }
            MemberKind::Constructor(ctor) => (
                ctor.name.name,
                BodyKind::Constructor,
                Some(ReceiverKind::RefMut),
                &ctor.params,
                Ty::Void,
            ),
            MemberKind::Method(method) => {
                method.body.as_ref()?;
                let ret = match &method.ret {
                    FnRetTy::Void(_) => Ty::Void,
                    FnRetTy::Ty(ty) => self.tcx.lower_ty(ty),
                };
                let receiver = method.receiver.map(|receiver| receiver.kind);
                (
                    method.name.name,
                    BodyKind::Method,
                    receiver,
                    &method.params,
                    ret,
                )
            }
        };

        if let Some(receiver) = receiver {
            let self_ty = self.tcx.self_ty(self.owner);
            let ty = match receiver {
                ReceiverKind::Owned => self_ty,
                ReceiverKind::Ref | ReceiverKind::RefMut => Ty::Ref {
                    mutable: receiver == ReceiverKind::RefMut,
                    ty: Box::new(self_ty),
                },
            };
            self.this = Some(self.push_local(kw::This, ty, member.span));
        }
        let params = params
            .iter()
            .map(|param| self.declare(param.name.name, param.name.span))
            .collect();

        let value = match &member.kind {
            MemberKind::Type(_) => return None,
            MemberKind::Field(field) => BodyValue::Expr(self.lower_expr(field.init.as_ref()?)),
            MemberKind::Constructor(ctor) => BodyValue::Block(self.lower_block(&ctor.body)),
            MemberKind::Method(method) => BodyValue::Block(self.lower_block(method.body.as_ref()?)),
        };
        Some(Body {
            owner: self.owner,
            name,
            kind,
            receiver,
            this: self.this,
            params,
            locals: self.locals,
            ret,
            value,
            span: member.span,
        })
    }

    fn push_local(&mut self, name: Symbol, ty: Ty, span: Span) -> LocalId {
        let id = LocalId(self.locals.len() as u32);
        self.locals.push(LocalDecl { name, ty, span });
        id
    }

    fn declare(&mut self, name: Symbol, decl: Span) -> LocalId {
        let ty = self
            .results
            .local_types
            .get(&decl)
            .cloned()
            .unwrap_or(Ty::Unknown);
        let id = self.push_local(name, ty, decl);
        self.by_decl.insert(decl, id);
        id
    }

    fn lower_block(&mut self, block: &ast::Block) -> Block {
        Block {
            stmts: block
                .stmts
                .iter()
                .filter_map(|stmt| self.lower_stmt(stmt))
                .collect(),
            span: block.span,
        }
    }

    fn lower_stmt_as_block(&mut self, stmt: &ast::Stmt) -> Block {
        match &stmt.kind {
            ast::StmtKind::Block(block) => self.lower_block(block),
            _ => Block {
                stmts: self.lower_stmt(stmt).into_iter().collect(),
                span: stmt.span,
            },
        }
    }

    fn lower_stmt(&mut self, stmt: &ast::Stmt) -> Option<Stmt> {
        let kind = match &stmt.kind {
            ast::StmtKind::Local(local) => {
                let init = local.init.as_ref().map(|init| self.lower_expr(init));
                StmtKind::Let {
                    local: self.declare(local.name.name, local.name.span),
                    init,
                }
            }
            ast::StmtKind::Expr(expr) => StmtKind::Expr(self.lower_expr(expr)),
            ast::StmtKind::Block(block) => StmtKind::Block(self.lower_block(block)),
            ast::StmtKind::If { cond, then, els } => StmtKind::If {
                cond: self.lower_expr(cond),
                then: self.lower_stmt_as_block(then),
                els: els.as_deref().map(|els| self.lower_stmt_as_block(els)),
            },
            ast::StmtKind::While { cond, body } => StmtKind::Loop {
                kind: LoopKind::While,
                cond: Some(self.lower_expr(cond)),
                update: Vec::new(),
                body: self.lower_stmt_as_block(body),
            },
            ast::StmtKind::DoWhile { body, cond } => StmtKind::Loop {
                kind: LoopKind::DoWhile,
                body: self.lower_stmt_as_block(body),
                cond: Some(self.lower_expr(cond)),
                update: Vec::new(),
            },
            ast::StmtKind::For {
                init,
                cond,
                update,
                body,
            } => {
                let mut stmts: Vec<Stmt> = init
                    .iter()
                    .filter_map(|stmt| self.lower_stmt(stmt))
                    .collect();
                let cond = cond.as_ref().map(|cond| self.lower_expr(cond));
                let update = update.iter().map(|expr| self.lower_expr(expr)).collect();
                let body = self.lower_stmt_as_block(body);
                stmts.push(Stmt {
                    kind: StmtKind::Loop {
                        kind: LoopKind::While,
                        cond,
                        update,
                        body,
                    },
                    span: stmt.span,
                });
                StmtKind::Block(Block {
                    stmts,
                    span: stmt.span,
                })
            }
            ast::StmtKind::ForEach {
                name, iter, body, ..
            } => StmtKind::ForEach {
                iter: self.lower_expr(iter),
                local: self.declare(name.name, name.span),
                body: self.lower_stmt_as_block(body),
            },
            ast::StmtKind::Switch { scrutinee, arms } => StmtKind::Switch {
                scrutinee: self.lower_expr(scrutinee),
                arms: arms
                    .iter()
                    .map(|arm| {
                        let (pats, guard) = self.lower_label(&arm.label);
                        Arm {
                            pats,
                            guard,
                            body: ArmBody::Block(self.lower_stmt_as_block(&arm.body)),
                            span: arm.span,
                        }
                    })
                    .collect(),
            },
            ast::StmtKind::Try {
                body,
                catches,
                finally,
            } => StmtKind::Try {
                body: self.lower_block(body),
                catches: catches
                    .iter()
                    .map(|catch| Catch {
                        local: self.declare(catch.name.name, catch.name.span),
                        tys: catch.tys.iter().map(|ty| self.tcx.lower_ty(ty)).collect(),
                        body: self.lower_block(&catch.body),
                    })
                    .collect(),
                finally: finally.as_ref().map(|block| self.lower_block(block)),
            },
            ast::StmtKind::Return(value) => {
                StmtKind::Return(value.as_ref().map(|value| self.lower_expr(value)))
            }
            ast::StmtKind::Yield(value) => StmtKind::Yield(self.lower_expr(value)),
            ast::StmtKind::Throw(value) => StmtKind::Throw(self.lower_expr(value)),
            ast::StmtKind::Break => StmtKind::Break,
            ast::StmtKind::Continue => StmtKind::Continue,
            ast::StmtKind::Empty => return None,
        };
        Some(Stmt {
            kind,
            span: stmt.span,
        })
    }

    fn lower_label(&mut self, label: &CaseLabel) -> (Vec<Pat>, Option<Expr>) {
        match label {
            CaseLabel::Default(span) => (
                vec![Pat {
                    kind: PatKind::Wild,
                    span: *span,
                }],
                None,
            ),
            CaseLabel::Case { pats, guard } => {
                let pats = pats.iter().map(|pat| self.lower_pat(pat)).collect();
                let guard = guard.as_ref().map(|guard| self.lower_expr(guard));
                (pats, guard)
            }
        }
    }

    fn lower_pat(&mut self, pat: &ast::Pat) -> Pat {
        let res = self.results.pats.get(&pat.span).cloned();
        let kind = match (&pat.kind, res) {
            (ast::PatKind::Wild, _) => PatKind::Wild,
            (ast::PatKind::Lit(expr), _) => PatKind::Lit(self.lower_expr(expr)),
            (ast::PatKind::Ty { binding, .. }, Some(PatRes::Type(ty))) => PatKind::Binding {
                local: binding.map(|binding| self.declare(binding.name, binding.span)),
                ty,
            },
            (ast::PatKind::Path(_), Some(PatRes::Variant { def, index })) => PatKind::Variant {
                def,
                index,
                fields: Vec::new(),
            },
            (ast::PatKind::Variant { fields, .. }, Some(PatRes::Variant { def, index })) => {
                PatKind::Variant {
                    def,
                    index,
                    fields: fields.iter().map(|field| self.lower_pat(field)).collect(),
                }
            }
            (ast::PatKind::Path(_), Some(PatRes::Type(ty))) => PatKind::Binding { local: None, ty },
            (ast::PatKind::Path(_), Some(PatRes::Const { owner, name, .. })) => {
                PatKind::Const { owner, name }
            }
            _ => PatKind::Wild,
        };
        Pat {
            kind,
            span: pat.span,
        }
    }

    fn expr_ty(&self, expr: &ast::Expr) -> Ty {
        self.results
            .expr_types
            .get(&expr.span)
            .cloned()
            .unwrap_or(Ty::Unknown)
    }

    fn callee(&self, span: Span) -> Option<Box<Callee>> {
        self.results.callees.get(&span).cloned().map(Box::new)
    }

    fn is_type_name(&self, expr: &ast::Expr) -> bool {
        let name = match &expr.kind {
            ast::ExprKind::Name(name) | ast::ExprKind::Field { name, .. } => name,
            _ => return false,
        };
        matches!(
            self.resolved.resolutions.get(name.span),
            Some(Res::Item(_) | Res::Import(_) | Res::TypeParam(_) | Res::PrimTy(_))
        )
    }

    fn lower_exprs(&mut self, exprs: &[ast::Expr]) -> Vec<Expr> {
        exprs.iter().map(|expr| self.lower_expr(expr)).collect()
    }

    fn lower_expr(&mut self, expr: &ast::Expr) -> Expr {
        let ty = self.expr_ty(expr);
        let kind = match &expr.kind {
            ast::ExprKind::Lit(lit) => ExprKind::Lit(*lit),
            ast::ExprKind::Bool(value) => ExprKind::Bool(*value),
            ast::ExprKind::Null => ExprKind::Null,
            ast::ExprKind::This | ast::ExprKind::Super => match self.this {
                Some(this) => return self.local_expr(this, expr.span),
                None => ExprKind::Unresolved,
            },
            ast::ExprKind::Name(name) => match self.resolved.resolutions.get(name.span) {
                Some(Res::Local(decl)) => match self.by_decl.get(&decl) {
                    Some(&local) => ExprKind::Local(local),
                    None => ExprKind::Unresolved,
                },
                _ => ExprKind::Unresolved,
            },
            ast::ExprKind::Field {
                expr: base,
                name,
                safe,
            } => match self.results.fields.get(&name.span) {
                Some(field) if self.is_type_name(base) => ExprKind::Static {
                    owner: field.owner,
                    name: name.name,
                },
                Some(field) => ExprKind::Field {
                    base: Box::new(self.lower_expr(base)),
                    owner: field.owner,
                    name: name.name,
                    safe: *safe,
                },
                None => {
                    let base = self.lower_expr(base);
                    match base.ty.strip_refs() {
                        Ty::Array(_) => ExprKind::Len {
                            array: Box::new(base),
                        },
                        _ => ExprKind::Unresolved,
                    }
                }
            },
            ast::ExprKind::MethodCall {
                receiver,
                name,
                args,
                safe,
            } => self.lower_method_call(receiver.as_deref(), name.span, args, *safe),
            ast::ExprKind::Call { callee, args } => match self.callee(callee.span) {
                Some(found) => {
                    let this = self.lower_expr(callee);
                    ExprKind::CtorCall {
                        args: self.lower_args(&found, args),
                        callee: found,
                        this: Box::new(this),
                    }
                }
                None => ExprKind::Unresolved,
            },
            ast::ExprKind::Index { expr: base, index } => ExprKind::Index {
                base: Box::new(self.lower_expr(base)),
                index: Box::new(self.lower_expr(index)),
            },
            ast::ExprKind::New { args, .. } => match self.callee(expr.span) {
                Some(found) => ExprKind::New {
                    args: self.lower_args(&found, args),
                    callee: Some(found),
                },
                None => ExprKind::New {
                    callee: None,
                    args: self.lower_exprs(args),
                },
            },
            ast::ExprKind::NewArray { len, .. } => ExprKind::NewArray {
                len: Box::new(self.lower_expr(len)),
            },
            ast::ExprKind::ArrayInit { elems, .. } => ExprKind::Array {
                elems: self.lower_exprs(elems),
            },
            ast::ExprKind::Unary { op, expr: operand } => ExprKind::Unary {
                op: *op,
                expr: Box::new(self.lower_expr(operand)),
            },
            ast::ExprKind::Borrow {
                mutable,
                expr: operand,
            } => ExprKind::Borrow {
                mutable: *mutable,
                expr: Box::new(self.lower_expr(operand)),
            },
            ast::ExprKind::Binary { op, lhs, rhs } => ExprKind::Binary {
                op: *op,
                lhs: Box::new(self.lower_expr(lhs)),
                rhs: Box::new(self.lower_expr(rhs)),
            },
            ast::ExprKind::Assign { op, lhs, rhs } => ExprKind::Assign {
                op: *op,
                place: Box::new(self.lower_expr(lhs)),
                value: Box::new(self.lower_expr(rhs)),
            },
            ast::ExprKind::Ternary { cond, then, els } => ExprKind::If {
                cond: Box::new(self.lower_expr(cond)),
                then: Box::new(self.lower_expr(then)),
                els: Box::new(self.lower_expr(els)),
            },
            ast::ExprKind::InstanceOf { expr: operand, pat } => ExprKind::Is {
                expr: Box::new(self.lower_expr(operand)),
                pat: Box::new(self.lower_pat(pat)),
            },
            ast::ExprKind::Cast { expr: operand, .. } => ExprKind::Cast {
                expr: Box::new(self.lower_expr(operand)),
            },
            ast::ExprKind::Lambda { .. } | ast::ExprKind::MethodRef { .. } => ExprKind::Closure,
            ast::ExprKind::Switch { scrutinee, arms } => ExprKind::Switch {
                scrutinee: Box::new(self.lower_expr(scrutinee)),
                arms: arms
                    .iter()
                    .map(|arm| {
                        let (pats, guard) = self.lower_label(&arm.label);
                        let body = match &arm.body {
                            SwitchExprBody::Expr(value) => ArmBody::Expr(self.lower_expr(value)),
                            SwitchExprBody::Block(block) => ArmBody::Block(self.lower_block(block)),
                            SwitchExprBody::Throw(value) => ArmBody::Throw(self.lower_expr(value)),
                        };
                        Arm {
                            pats,
                            guard,
                            body,
                            span: arm.span,
                        }
                    })
                    .collect(),
            },
        };
        Expr {
            kind,
            ty,
            span: expr.span,
        }
    }

    fn lower_method_call(
        &mut self,
        receiver: Option<&ast::Expr>,
        name: Span,
        args: &[ast::Expr],
        safe: bool,
    ) -> ExprKind {
        let Some(callee) = self.callee(name) else {
            return ExprKind::Unresolved;
        };
        let dispatch = match receiver.map(|receiver| &receiver.kind) {
            Some(ast::ExprKind::Super) => Dispatch::Static,
            _ if callee.method.receiver.is_none() => Dispatch::Static,
            _ => Dispatch::Virtual,
        };
        let receiver = match (receiver, callee.method.receiver) {
            (Some(base), Some(kind)) if !self.is_type_name(base) => {
                let base = self.lower_expr(base);
                Some(Box::new(auto_ref(base, kind)))
            }
            (None, Some(kind)) => self.this.map(|this| {
                let this = self.local_expr(this, name);
                Box::new(auto_ref(this, kind))
            }),
            _ => None,
        };
        ExprKind::Call {
            args: self.lower_args(&callee, args),
            callee,
            receiver,
            dispatch,
            safe,
        }
    }

    fn local_expr(&self, local: LocalId, span: Span) -> Expr {
        Expr {
            kind: ExprKind::Local(local),
            ty: self.locals[local.index()].ty.clone(),
            span,
        }
    }

    fn lower_args(&mut self, callee: &Callee, args: &[ast::Expr]) -> Vec<Expr> {
        let mut lowered = self.lower_exprs(args);
        if !callee.packs_varargs {
            return lowered;
        }
        let fixed = callee.params.len().saturating_sub(1);
        let rest: Vec<Expr> = lowered.split_off(fixed.min(lowered.len()));
        let span = match (rest.first(), rest.last()) {
            (Some(first), Some(last)) => first.span.to(last.span),
            _ => lowered
                .last()
                .map_or(Span::DUMMY, |arg| arg.span.shrink_to_hi()),
        };
        lowered.push(Expr {
            kind: ExprKind::Array { elems: rest },
            ty: callee.params.last().cloned().unwrap_or(Ty::Unknown),
            span,
        });
        lowered
    }
}

fn auto_ref(expr: Expr, receiver: ReceiverKind) -> Expr {
    let mutable = match receiver {
        ReceiverKind::Owned => return expr,
        ReceiverKind::Ref => false,
        ReceiverKind::RefMut => true,
    };
    if let Ty::Ref { .. } = expr.ty {
        return expr;
    }
    Expr {
        ty: Ty::Ref {
            mutable,
            ty: Box::new(expr.ty.clone()),
        },
        span: expr.span,
        kind: ExprKind::Borrow {
            mutable,
            expr: Box::new(expr),
        },
    }
}

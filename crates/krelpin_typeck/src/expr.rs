use krelpin_ast::ast::{
    BinOp, Expr, ExprKind, FieldStorage, Ident, PrimitiveTy, SwitchExprBody, UnOp, Visibility,
};
use krelpin_ast::token::{Lit, LitKind};
use krelpin_errors::Diagnostic;
use krelpin_resolve::Res;
use krelpin_span::def_id::DefId;
use krelpin_span::{Span, Symbol};

use crate::check::{FnCtxt, plural};
use crate::closure::is_closure;
use crate::lang_items::LangItem;
use crate::sig::{AdtKind, FieldSig};
use crate::ty::{Ty, promote};

enum Receiver {
    None,
    Type(Ty),
    Value(Ty),
}

impl FnCtxt<'_, '_> {
    pub(crate) fn check_expr(&mut self, expr: &Expr) -> Ty {
        let ty = self.check_expr_kind(expr);
        self.results.insert(expr.span, ty.clone());
        ty
    }

    pub(crate) fn check_expr_coerce(&mut self, expr: &Expr, expected: &Ty) -> Ty {
        if is_closure(expr) {
            return self.check_closure(expr, expected);
        }
        if let ExprKind::ArrayInit { ty: None, elems } = &expr.kind {
            let Ty::Array(elem) = expected.non_null().strip_refs() else {
                if !expected.is_unknown() {
                    self.error(
                        expr.span,
                        format!("an array initializer cannot be used as `{expected}`"),
                    );
                }
                return Ty::Unknown;
            };
            let elem = elem.as_ref().clone();
            for value in elems {
                self.check_expr_coerce(value, &elem);
            }
            self.results.insert(expr.span, expected.clone());
            return expected.clone();
        }

        let actual = self.check_expr(expr);
        self.require_assignable(expr, &actual, expected);
        actual
    }

    pub(crate) fn require_assignable(&self, expr: &Expr, actual: &Ty, expected: &Ty) {
        if self.accepts(expr, actual, expected) || narrows_literal(expr, expected) {
            return;
        }
        self.mismatch(expr.span, expected, actual);
    }

    fn check_expr_kind(&mut self, expr: &Expr) -> Ty {
        match &expr.kind {
            ExprKind::Lit(lit) => self.lit_ty(lit),
            ExprKind::Bool(_) => Ty::BOOLEAN,
            ExprKind::Null => Ty::Null,
            ExprKind::This => self.self_ty.clone(),
            ExprKind::Super => self
                .tcx
                .sig(self.owner)
                .superclass
                .clone()
                .unwrap_or_else(|| self.tcx.lang_ty(LangItem::Object)),
            ExprKind::Name(name) => self.check_name(*name),
            ExprKind::Field {
                expr: base,
                name,
                safe,
            } => self.field_access(base, *name, *safe).0,
            ExprKind::MethodCall {
                receiver,
                name,
                args,
                safe,
            } => self.check_method_call(receiver.as_deref(), *name, args, *safe),
            ExprKind::Call { callee, args } => {
                let arg_tys = self.check_closure_args(args);
                let target = match callee.kind {
                    ExprKind::Super => self.tcx.sig(self.owner).superclass.clone(),
                    _ => Some(self.tcx.self_ty(self.owner)),
                };
                if let Some(Ty::Adt {
                    def, args: ty_args, ..
                }) = target
                {
                    let name = Ident::new(self.type_name(def), callee.span);
                    self.check_ctor_call(def, &ty_args, args, &arg_tys, name);
                }
                self.finish_closure_args(args);
                Ty::Void
            }
            ExprKind::Index { expr: base, index } => self.check_index(base, index),
            ExprKind::New { ty, args } => self.check_new(ty, args, expr.span),
            ExprKind::NewArray { elem, len } => {
                self.check_expr_coerce(len, &Ty::INT);
                Ty::Array(Box::new(self.lower_ty(elem)))
            }
            ExprKind::ArrayInit { ty, elems } => {
                let ty = ty.as_ref().map_or(Ty::Unknown, |ty| self.lower_ty(ty));
                if ty.is_unknown() {
                    self.error(
                        expr.span,
                        "an array initializer needs an explicit array type here",
                    );
                    return Ty::Unknown;
                }
                let elem = match &ty {
                    Ty::Array(elem) => elem.as_ref().clone(),
                    _ => Ty::Unknown,
                };
                for value in elems {
                    self.check_expr_coerce(value, &elem);
                }
                ty
            }
            ExprKind::Unary { op, expr: operand } => self.check_unary(*op, operand, expr.span),
            ExprKind::Borrow {
                mutable,
                expr: operand,
            } => Ty::Ref {
                mutable: *mutable,
                ty: Box::new(self.check_expr(operand)),
            },
            ExprKind::Binary { op, lhs, rhs } => self.check_binary(*op, lhs, rhs, expr.span),
            ExprKind::Assign { op, lhs, rhs } => self.check_assign(*op, lhs, rhs, expr.span),
            ExprKind::Ternary { cond, then, els } => {
                self.check_expr_coerce(cond, &Ty::BOOLEAN);
                let (when_true, when_false) = self.null_checks(cond);
                let then_ty = self.with_narrowing(&when_true, |fcx| fcx.check_expr(then));
                let els_ty = self.with_narrowing(&when_false, |fcx| fcx.check_expr(els));
                self.common_or_error(&then_ty, &els_ty, expr.span, "the branches of `?`")
            }
            ExprKind::InstanceOf {
                expr: operand,
                ty,
                binding,
            } => {
                self.check_expr(operand);
                let ty = self.lower_ty(ty);
                if let Some(binding) = binding {
                    self.locals.insert(binding.span, ty);
                }
                Ty::BOOLEAN
            }
            ExprKind::Cast { ty, expr: operand } => {
                let from = self.check_expr(operand);
                let to = self.lower_ty(ty);
                self.check_cast(&from, &to, expr.span);
                to
            }
            ExprKind::Lambda { .. } | ExprKind::MethodRef { .. } => {
                self.check_closure(expr, &Ty::Unknown)
            }
            ExprKind::Switch { scrutinee, arms } => {
                let scrutinee_ty = self.check_expr(scrutinee);
                let mut arm_tys = Vec::new();
                for arm in arms {
                    self.check_case_labels(arm.labels.as_deref(), &scrutinee_ty);
                    match &arm.body {
                        SwitchExprBody::Expr(value) => {
                            arm_tys.push((value.span, self.check_expr(value)))
                        }
                        SwitchExprBody::Throw(value) => {
                            self.check_throw(value);
                        }
                        SwitchExprBody::Block(block) => {
                            self.yields.push(Vec::new());
                            self.check_block(block);
                            let yields = self.yields.pop().unwrap_or_default();
                            arm_tys.extend(yields.into_iter().map(|ty| (block.span, ty)));
                        }
                    }
                }
                self.check_switch_exhaustive(
                    arms.iter().map(|arm| arm.labels.as_deref()),
                    &scrutinee_ty,
                    expr.span,
                );

                let mut result: Option<Ty> = None;
                for (span, ty) in arm_tys {
                    result = Some(match result {
                        None => ty,
                        Some(previous) => {
                            self.common_or_error(&previous, &ty, span, "the cases of this `switch`")
                        }
                    });
                }
                result.unwrap_or(Ty::Unknown)
            }
        }
    }

    pub(crate) fn type_of_name(&self, expr: &Expr) -> Option<Ty> {
        let ExprKind::Name(name) = expr.kind else {
            return None;
        };
        match self.tcx.resolutions.get(name.span)? {
            Res::Item(def) => Some(Ty::Adt {
                def,
                name: name.name,
                args: Vec::new(),
            }),
            Res::Import(_) => Some(Ty::External {
                name: name.name,
                args: Vec::new(),
            }),
            Res::TypeParam(_) => Some(Ty::Param(name.name)),
            Res::PrimTy(primitive) => Some(Ty::Primitive(primitive)),
            Res::Local(_) | Res::BorrowSource(_) => None,
        }
    }

    fn check_name(&mut self, name: Ident) -> Ty {
        match self.tcx.resolutions.get(name.span) {
            Some(Res::Local(decl)) => self.local_ty(decl),
            Some(Res::PrimTy(_)) => {
                self.error(
                    name.span,
                    format!("expected a value, found type `{}`", name.name),
                );
                Ty::Unknown
            }
            Some(Res::Item(_) | Res::Import(_) | Res::TypeParam(_)) => {
                self.emit(
                    Diagnostic::error(format!("expected a value, found type `{}`", name.name))
                        .with_span(name.span)
                        .with_help(format!(
                            "to create an object, write `new {}(...)`",
                            name.name
                        )),
                );
                Ty::Unknown
            }
            _ => Ty::Unknown,
        }
    }

    fn receiver(&mut self, base: &Expr, safe: bool) -> (Receiver, bool) {
        if let Some(ty) = self.type_of_name(base) {
            return (Receiver::Type(ty), false);
        }

        let ty = self.check_expr(base);
        match ty {
            Ty::Nullable(inner) => {
                if !safe {
                    self.emit(
                        Diagnostic::error(format!("value of type `{}?` may be null", inner))
                            .with_span(base.span)
                            .with_help(
                                "use `?.` to access it only when it is not null, \
                                 or check `!= null` first",
                            ),
                    );
                }
                (Receiver::Value(*inner), safe)
            }
            Ty::Null => {
                self.error(base.span, "`null` has no fields or methods");
                (Receiver::Value(Ty::Unknown), false)
            }
            ty => {
                if safe && !ty.is_unknown() {
                    self.emit(
                        Diagnostic::warning("unnecessary `?.` on a value that is never null")
                            .with_span(base.span)
                            .with_help("use `.` instead"),
                    );
                }
                (Receiver::Value(ty), false)
            }
        }
    }

    pub(crate) fn field_access(
        &mut self,
        base: &Expr,
        name: Ident,
        safe: bool,
    ) -> (Ty, Option<FieldSig>) {
        let (receiver, nullable) = self.receiver(base, safe);
        let (ty, field) = match receiver {
            Receiver::Type(ty) => self.static_field(&ty, name),
            Receiver::Value(ty) => self.instance_field(ty.strip_refs(), name),
            Receiver::None => (Ty::Unknown, None),
        };
        let ty = if nullable { ty.nullable() } else { ty };
        (ty, field)
    }

    fn static_field(&mut self, ty: &Ty, name: Ident) -> (Ty, Option<FieldSig>) {
        let Some(def) = self.static_def(ty) else {
            return (Ty::Unknown, None);
        };
        let Some((field, field_ty)) = self.find_field(def, &[], name.name) else {
            self.no_field(ty, name);
            return (Ty::Unknown, None);
        };
        if field.storage == FieldStorage::Instance {
            self.emit(
                Diagnostic::error(format!("`{}` is a field of each object", name.name))
                    .with_span(name.span)
                    .with_help(format!(
                        "access it through an object of `{}`, like `this.{}`",
                        self.type_name(field.owner),
                        name.name
                    )),
            );
        }
        self.check_vis(field.vis, field.owner, true, "field", name);
        (field_ty, Some(field))
    }

    fn instance_field(&mut self, ty: &Ty, name: Ident) -> (Ty, Option<FieldSig>) {
        match ty {
            Ty::Adt { .. } | Ty::Param(_) | Ty::Primitive(_) => {
                let found = self
                    .receiver_adts(ty)
                    .into_iter()
                    .find_map(|(def, args)| self.find_field(def, &args, name.name));
                let Some((field, field_ty)) = found else {
                    self.no_field(ty, name);
                    return (Ty::Unknown, None);
                };
                if field.storage != FieldStorage::Instance {
                    let owner = self.type_name(field.owner);
                    self.emit(
                        Diagnostic::error(format!(
                            "`{}` belongs to the class `{owner}`",
                            name.name
                        ))
                        .with_span(name.span)
                        .with_help(format!("write `{owner}.{}`", name.name)),
                    );
                }
                self.check_vis(field.vis, field.owner, true, "field", name);
                (field_ty, Some(field))
            }
            Ty::Array(_) if name.name.as_str() == "length" => (Ty::INT, None),
            Ty::Array(_) | Ty::Void => {
                self.no_field(ty, name);
                (Ty::Unknown, None)
            }
            _ => (Ty::Unknown, None),
        }
    }

    fn no_field(&self, ty: &Ty, name: Ident) {
        self.emit(
            Diagnostic::error(format!("no field `{}` on type `{ty}`", name.name))
                .with_span(name.span)
                .with_label("unknown field"),
        );
    }

    fn find_field(&self, def: DefId, args: &[Ty], name: Symbol) -> Option<(FieldSig, Ty)> {
        let sig = self.tcx.sig(def);
        let subst = self.tcx.subst_for(def, args);
        if let Some(field) = sig.fields.get(&name) {
            return Some((field.clone(), field.ty.subst(&subst)));
        }
        match &sig.superclass {
            Some(Ty::Adt {
                def: parent, args, ..
            }) => {
                let args: Vec<Ty> = args.iter().map(|arg| arg.subst(&subst)).collect();
                self.find_field(*parent, &args, name)
            }
            _ => None,
        }
    }

    pub(crate) fn check_vis(
        &self,
        vis: Visibility,
        owner: DefId,
        default_private: bool,
        what: &str,
        name: Ident,
    ) {
        let level = match vis {
            Visibility::Public(_) => return,
            Visibility::Default if !default_private => return,
            Visibility::Default | Visibility::Private(_) => "private",
            Visibility::Protected(_) => {
                if self.tcx.is_subclass(self.owner, owner) {
                    return;
                }
                "protected"
            }
        };
        if self.owner == owner {
            return;
        }
        self.emit(
            Diagnostic::error(format!(
                "{what} `{}` of `{}` is {level}",
                name.name,
                self.type_name(owner)
            ))
            .with_span(name.span)
            .with_label(format!("{level} {what}")),
        );
    }

    fn check_method_call(
        &mut self,
        receiver: Option<&Expr>,
        name: Ident,
        args: &[Expr],
        safe: bool,
    ) -> Ty {
        let ty = self.method_call_ty(receiver, name, args, safe);
        self.finish_closure_args(args);
        ty
    }

    fn method_call_ty(
        &mut self,
        receiver: Option<&Expr>,
        name: Ident,
        args: &[Expr],
        safe: bool,
    ) -> Ty {
        let (receiver, nullable) = match receiver {
            Some(base) => self.receiver(base, safe),
            None => (Receiver::None, false),
        };
        let arg_tys = self.check_closure_args(args);

        let ret = match receiver {
            Receiver::None => {
                let owner = self.owner;
                let candidates = self.tcx.methods_named(owner, &[], name.name);
                let statics: Vec<_> = candidates
                    .into_iter()
                    .filter(|(method, _)| method.receiver.is_none())
                    .collect();
                if statics.is_empty() {
                    Ty::Unknown
                } else {
                    self.call(statics, args, &arg_tys, name, "method")
                }
            }
            Receiver::Type(ty) => self.static_call(&ty, name, args, &arg_tys),
            Receiver::Value(ty) => self.instance_call(ty.strip_refs(), name, args, &arg_tys),
        };

        if nullable { ret.nullable() } else { ret }
    }

    fn static_call(&mut self, ty: &Ty, name: Ident, args: &[Expr], arg_tys: &[Ty]) -> Ty {
        let Some(def) = self.static_def(ty) else {
            return Ty::Unknown;
        };

        if self.tcx.sig(def).kind == AdtKind::Enum
            && name.name.as_str() == "values"
            && args.is_empty()
        {
            return Ty::Array(Box::new(ty.clone()));
        }

        let candidates = self.tcx.methods_named(def, &[], name.name);
        if candidates.is_empty() {
            self.no_method(ty, name);
            return Ty::Unknown;
        }
        let statics: Vec<_> = candidates
            .into_iter()
            .filter(|(method, _)| method.receiver.is_none())
            .collect();
        if statics.is_empty() {
            self.emit(
                Diagnostic::error(format!(
                    "method `{}` needs an object to be called on",
                    name.name
                ))
                .with_span(name.span)
                .with_help(format!("call it on an object of `{ty}`")),
            );
            return Ty::Unknown;
        }
        self.call(statics, args, arg_tys, name, "method")
    }

    fn instance_call(&mut self, ty: &Ty, name: Ident, args: &[Expr], arg_tys: &[Ty]) -> Ty {
        match ty {
            Ty::Adt { .. } | Ty::Param(_) | Ty::Primitive(_) => {
                let receivers = self.receiver_adts(ty);
                let candidates: Vec<_> = receivers
                    .iter()
                    .flat_map(|(def, args)| self.tcx.methods_named(*def, args, name.name))
                    .collect();
                if candidates.is_empty() {
                    self.no_method(ty, name);
                    return Ty::Unknown;
                }
                let instance: Vec<_> = candidates
                    .into_iter()
                    .filter(|(method, _)| method.receiver.is_some())
                    .collect();
                if instance.is_empty() {
                    let owner = self.type_name(receivers[0].0);
                    self.emit(
                        Diagnostic::error(format!(
                            "`{}` belongs to the class `{owner}`",
                            name.name
                        ))
                        .with_span(name.span)
                        .with_help(format!("call it as `{owner}.{}(...)`", name.name)),
                    );
                    return Ty::Unknown;
                }
                self.call(instance, args, arg_tys, name, "method")
            }
            Ty::Void => {
                self.no_method(ty, name);
                Ty::Unknown
            }
            _ => Ty::Unknown,
        }
    }

    fn static_def(&self, ty: &Ty) -> Option<DefId> {
        match ty {
            Ty::Adt { def, .. } => Some(*def),
            Ty::Primitive(primitive) => self.tcx.lang_items.primitive_impl(*primitive),
            _ => None,
        }
    }

    pub(crate) fn receiver_adts(&self, ty: &Ty) -> Vec<(DefId, Vec<Ty>)> {
        match ty {
            Ty::Adt { def, args, .. } => vec![(*def, args.clone())],
            Ty::Primitive(primitive) => {
                let def = self.tcx.lang_items.primitive_impl(*primitive);
                def.into_iter().map(|def| (def, Vec::new())).collect()
            }
            Ty::Param(name) => {
                let bounds: Vec<_> = self
                    .param_env
                    .bounds(*name)
                    .iter()
                    .flat_map(|bound| self.receiver_adts(bound))
                    .collect();
                if !bounds.is_empty() {
                    return bounds;
                }
                let object = self.tcx.lang_items.get(LangItem::Object);
                object.into_iter().map(|def| (def, Vec::new())).collect()
            }
            _ => Vec::new(),
        }
    }

    fn no_method(&self, ty: &Ty, name: Ident) {
        self.emit(
            Diagnostic::error(format!("no method named `{}` found for `{ty}`", name.name))
                .with_span(name.span)
                .with_label("method not found"),
        );
    }

    pub(crate) fn check_ctor_call(
        &mut self,
        def: DefId,
        ty_args: &[Ty],
        args: &[Expr],
        arg_tys: &[Ty],
        name: Ident,
    ) {
        let ctors = self.tcx.sig(def).ctors.clone();
        if ctors.is_empty() {
            if !args.is_empty() {
                self.error(
                    name.span,
                    format!(
                        "`{}` has no constructor that takes {}",
                        self.type_name(def),
                        plural(args.len(), "argument")
                    ),
                );
            }
            return;
        }
        let subst = self.tcx.subst_for(def, ty_args);
        let candidates = ctors
            .into_iter()
            .map(|ctor| (ctor, subst.clone()))
            .collect();
        self.call(candidates, args, arg_tys, name, "constructor");
    }

    fn check_new(&mut self, ty: &krelpin_ast::ast::Ty, args: &[Expr], span: Span) -> Ty {
        let ty = self.new_object_ty(ty, args, span);
        self.finish_closure_args(args);
        ty
    }

    fn new_object_ty(&mut self, ty: &krelpin_ast::ast::Ty, args: &[Expr], span: Span) -> Ty {
        let ty = self.lower_ty(ty);
        let arg_tys = self.check_closure_args(args);

        match &ty {
            Ty::Adt {
                def,
                name,
                args: ty_args,
            } => {
                let sig = self.tcx.sig(*def);
                let problem = match sig.kind {
                    AdtKind::Interface => Some("an interface"),
                    AdtKind::Enum => Some("an enum"),
                    AdtKind::Class if sig.is_abstract => Some("an abstract class"),
                    _ => None,
                };
                if let Some(problem) = problem {
                    self.error(
                        span,
                        format!("cannot create an object of {problem} `{name}`"),
                    );
                    return ty;
                }
                let name = Ident::new(*name, span);
                self.check_ctor_call(*def, ty_args, args, &arg_tys, name);
            }
            Ty::Param(name) => {
                self.error(
                    span,
                    format!("cannot create an object of generic parameter `{name}`"),
                );
            }
            _ => {}
        }
        ty
    }

    fn check_index(&mut self, base: &Expr, index: &Expr) -> Ty {
        let base_ty = self.check_expr(base);
        self.check_expr_coerce(index, &Ty::INT);
        match base_ty.strip_refs() {
            Ty::Array(elem) => elem.as_ref().clone(),
            Ty::Unknown | Ty::External { .. } | Ty::Param(_) => Ty::Unknown,
            Ty::Nullable(_) => {
                self.error(base.span, format!("value of type `{base_ty}` may be null"));
                Ty::Unknown
            }
            other => {
                let diagnostic =
                    Diagnostic::error(format!("cannot index into a value of type `{other}`"))
                        .with_span(base.span);
                let has_get = match other {
                    Ty::Adt { def, args, .. } => !self
                        .tcx
                        .methods_named(*def, args, Symbol::intern("get"))
                        .is_empty(),
                    _ => false,
                };
                let diagnostic = if has_get {
                    diagnostic.with_help("use `.get(index)` instead")
                } else {
                    diagnostic
                };
                self.emit(diagnostic);
                Ty::Unknown
            }
        }
    }

    fn check_unary(&mut self, op: UnOp, operand: &Expr, span: Span) -> Ty {
        let ty = match op {
            UnOp::PreInc | UnOp::PreDec | UnOp::PostInc | UnOp::PostDec => {
                self.check_place(operand)
            }
            UnOp::Neg | UnOp::Not | UnOp::BitNot => self.check_expr(operand),
        };
        let value = ty.strip_refs().clone();
        if value.is_unknown() {
            return Ty::Unknown;
        }

        let ok = match op {
            UnOp::Not => value.is_boolean(),
            UnOp::BitNot => value.is_integral(),
            _ => value.is_numeric(),
        };
        if !ok {
            let symbol = match op {
                UnOp::Neg => "-",
                UnOp::Not => "!",
                UnOp::BitNot => "~",
                UnOp::PreInc | UnOp::PostInc => "++",
                UnOp::PreDec | UnOp::PostDec => "--",
            };
            self.error(span, format!("cannot apply `{symbol}` to `{value}`"));
            return Ty::Unknown;
        }

        match (op, value.primitive()) {
            (UnOp::Neg | UnOp::BitNot, Some(primitive)) => promote(primitive, PrimitiveTy::Int),
            _ => value,
        }
    }

    fn check_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr, span: Span) -> Ty {
        if matches!(op, BinOp::And | BinOp::Or) {
            self.check_expr_coerce(lhs, &Ty::BOOLEAN);
            let (when_true, when_false) = self.null_checks(lhs);
            let narrowing = if op == BinOp::And {
                when_true
            } else {
                when_false
            };
            self.with_narrowing(&narrowing, |fcx| fcx.check_expr_coerce(rhs, &Ty::BOOLEAN));
            return Ty::BOOLEAN;
        }

        let left = self.check_expr(lhs);
        let right = self.check_expr(rhs);

        if op == BinOp::Elvis {
            return match &left {
                Ty::Nullable(inner) => {
                    self.common_or_error(inner, &right, span, "the sides of `?:`")
                }
                Ty::Null | Ty::Unknown => right,
                other => {
                    self.emit(
                        Diagnostic::warning("the left side of `?:` can never be null")
                            .with_span(lhs.span)
                            .with_help("remove the `?:` and the default value"),
                    );
                    self.common_or_error(other, &right, span, "the sides of `?:`")
                }
            };
        }

        let (l, r) = (left.strip_refs(), right.strip_refs());
        let is_comparison = matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        );
        if l.is_unknown() || r.is_unknown() {
            return if is_comparison {
                Ty::BOOLEAN
            } else if op == BinOp::Add && (self.tcx.is_string(l) || self.tcx.is_string(r)) {
                self.tcx.string_ty()
            } else {
                Ty::Unknown
            };
        }

        let both = |pred: fn(&Ty) -> bool| pred(l) && pred(r);
        let result = match op {
            BinOp::Eq | BinOp::Ne => self.comparable(l, r).then_some(Ty::BOOLEAN),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                both(Ty::is_numeric).then_some(Ty::BOOLEAN)
            }
            BinOp::Add if self.tcx.is_string(l) || self.tcx.is_string(r) => {
                Some(self.tcx.string_ty())
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => numeric_result(l, r),
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor if both(Ty::is_boolean) => {
                Some(Ty::BOOLEAN)
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => both(Ty::is_integral)
                .then(|| numeric_result(l, r))
                .flatten(),
            BinOp::Shl | BinOp::Shr | BinOp::UShr => both(Ty::is_integral)
                .then(|| l.primitive().map(|p| promote(p, PrimitiveTy::Int)))
                .flatten(),
            BinOp::And | BinOp::Or | BinOp::Elvis => None,
        };

        result.unwrap_or_else(|| {
            let message = if matches!(op, BinOp::Eq | BinOp::Ne) {
                format!("cannot compare `{l}` with `{r}`")
            } else {
                format!("cannot apply `{}` to `{l}` and `{r}`", op.as_str())
            };
            self.error(span, message);
            Ty::Unknown
        })
    }

    fn comparable(&self, l: &Ty, r: &Ty) -> bool {
        match (l, r) {
            (Ty::Primitive(_), Ty::Primitive(_)) => {
                (l.is_numeric() && r.is_numeric()) || (l.is_boolean() && r.is_boolean())
            }
            (Ty::Null, Ty::Primitive(_)) | (Ty::Primitive(_), Ty::Null) => false,
            (Ty::Null, _) | (_, Ty::Null) => true,
            _ => self.is_assignable(l, r) || self.is_assignable(r, l),
        }
    }

    fn check_assign(&mut self, op: Option<BinOp>, lhs: &Expr, rhs: &Expr, span: Span) -> Ty {
        let target = self.check_place(lhs);
        let assigned_local = match lhs.kind {
            ExprKind::Name(name) => match self.tcx.resolutions.get(name.span) {
                Some(Res::Local(decl)) => Some(decl),
                _ => None,
            },
            _ => None,
        };
        if let Some(decl) = assigned_local {
            for scope in &mut self.narrowed {
                scope.remove(&decl);
            }
        }

        match op {
            None => {
                self.check_expr_coerce(rhs, &target);
            }
            Some(op) => {
                let value = self.check_expr(rhs);
                let target_value = target.strip_refs();
                let ok = target_value.is_unknown()
                    || value.is_unknown()
                    || (op == BinOp::Add && self.tcx.is_string(target_value))
                    || match op {
                        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                            (target_value.is_boolean() && value.is_boolean())
                                || (target_value.is_integral() && value.is_integral())
                        }
                        BinOp::Shl | BinOp::Shr | BinOp::UShr => {
                            target_value.is_integral() && value.is_integral()
                        }
                        _ => target_value.is_numeric() && value.is_numeric(),
                    };
                if !ok {
                    self.error(
                        span,
                        format!(
                            "cannot apply `{}=` to `{target_value}` and `{value}`",
                            op.as_str()
                        ),
                    );
                }
            }
        }
        target
    }

    fn check_place(&mut self, expr: &Expr) -> Ty {
        match &expr.kind {
            ExprKind::Name(name) => match self.tcx.resolutions.get(name.span) {
                Some(Res::Local(decl)) => {
                    let ty = self.locals.get(&decl).cloned().unwrap_or(Ty::Unknown);
                    self.results.insert(expr.span, ty.clone());
                    ty
                }
                _ => self.check_expr(expr),
            },
            ExprKind::Index { .. } => self.check_expr(expr),
            ExprKind::Field {
                expr: base,
                name,
                safe,
            } => {
                if *safe {
                    self.error(expr.span, "cannot assign through `?.`");
                }
                let (ty, field) = self.field_access(base, *name, *safe);
                self.results.insert(expr.span, ty.clone());
                if let Some(field) = field {
                    self.check_field_assignable(&field, *name);
                }
                ty
            }
            _ => {
                let ty = self.check_expr(expr);
                self.error(expr.span, "invalid left-hand side of assignment");
                ty
            }
        }
    }

    fn check_field_assignable(&self, field: &FieldSig, name: Ident) {
        match field.storage {
            FieldStorage::Instance => {}
            FieldStorage::Const(_) => {
                self.error(
                    name.span,
                    format!("cannot assign to `const` field `{}`", name.name),
                );
            }
            FieldStorage::Shared(_) => self.emit(
                Diagnostic::error(format!("cannot assign to `shared` field `{}`", name.name))
                    .with_span(name.span)
                    .with_note("a `shared` field can be used from many places at once")
                    .with_help(
                        "store a type that can be changed safely, like `AtomicInt` or `Mutex<T>`",
                    ),
            ),
        }
    }

    fn check_cast(&self, from: &Ty, to: &Ty, span: Span) {
        let (from_value, to_value) = (from.strip_refs(), to.strip_refs());
        let invalid = match (from_value.primitive(), to_value.primitive()) {
            (Some(a), Some(b)) => (a == PrimitiveTy::Boolean) != (b == PrimitiveTy::Boolean),
            (Some(_), None) | (None, Some(_)) => {
                !(from_value.is_unknown()
                    || to_value.is_unknown()
                    || self.is_assignable(from_value, to_value)
                    || self.is_assignable(to_value, from_value))
            }
            (None, None) => false,
        };
        if invalid {
            self.error(span, format!("cannot cast `{from}` to `{to}`"));
        }
    }

    pub(crate) fn common_or_error(&self, a: &Ty, b: &Ty, span: Span, what: &str) -> Ty {
        if a.is_unknown() || b.is_unknown() {
            return if a.is_unknown() { b.clone() } else { a.clone() };
        }
        self.tcx
            .common_ty(&self.param_env, a, b)
            .unwrap_or_else(|| {
                self.error(
                    span,
                    format!("{what} have incompatible types `{a}` and `{b}`"),
                );
                Ty::Unknown
            })
    }

    fn lit_ty(&self, lit: &Lit) -> Ty {
        let suffix = lit.suffix.map(Symbol::as_str);
        match lit.kind {
            LitKind::Integer => match suffix {
                Some("L" | "l") => Ty::LONG,
                _ => Ty::INT,
            },
            LitKind::Float => match suffix {
                Some("f" | "F") => Ty::FLOAT,
                _ => Ty::DOUBLE,
            },
            LitKind::Char => Ty::CHAR,
            LitKind::Str | LitKind::TextBlock => self.tcx.string_ty(),
            LitKind::Err => Ty::Unknown,
        }
    }
}

fn numeric_result(l: &Ty, r: &Ty) -> Option<Ty> {
    match (l.primitive(), r.primitive()) {
        (Some(a), Some(b)) if l.is_numeric() && r.is_numeric() => Some(promote(a, b)),
        _ => None,
    }
}

fn narrows_literal(expr: &Expr, expected: &Ty) -> bool {
    is_int_literal(expr)
        && matches!(
            expected.non_null(),
            Ty::Primitive(PrimitiveTy::Byte | PrimitiveTy::Short | PrimitiveTy::Char)
        )
}

fn is_int_literal(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Lit(lit) => lit.kind == LitKind::Integer && lit.suffix.is_none(),
        ExprKind::Unary {
            op: UnOp::Neg,
            expr,
        } => is_int_literal(expr),
        _ => false,
    }
}

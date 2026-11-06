use krelpin_ast::ast::{Block, Expr, ExprKind, Ident, LambdaBody, LambdaParam, ReceiverKind};
use krelpin_errors::Diagnostic;
use krelpin_span::def_id::DefId;
use krelpin_span::{Span, Symbol, kw};

use crate::Callee;
use crate::check::{FnCtxt, plural};
use crate::exceptions::ExceptionScope;
use crate::lang_items::LangItem;
use crate::sig::{AdtKind, MethodSig, Tcx};
use crate::ty::{Subst, Ty};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnSig {
    pub name: Symbol,
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub throws: Vec<Ty>,
}

impl Tcx<'_> {
    pub fn fn_sig(&self, ty: &Ty) -> Option<FnSig> {
        let Ty::Adt { def, args, .. } = ty else {
            return None;
        };
        if self.sig(*def).kind != AdtKind::Interface {
            return None;
        }
        match self.abstract_methods(*def, args).as_slice() {
            [(method, sig)] if method.generics.is_empty() => Some(sig.clone()),
            _ => None,
        }
    }

    pub fn fn_receiver(&self, ty: &Ty) -> Option<ReceiverKind> {
        let Ty::Adt { def, args, .. } = ty else {
            return None;
        };
        match self.abstract_methods(*def, args).as_slice() {
            [(method, _)] => method.receiver,
            _ => None,
        }
    }

    pub(crate) fn abstract_methods(&self, def: DefId, args: &[Ty]) -> Vec<(MethodSig, FnSig)> {
        let mut methods = Vec::new();
        self.collect_abstract_methods(def, args, &mut methods);
        methods.sort_by_key(|(method, _)| method.name.span.lo);
        methods
    }

    fn collect_abstract_methods(&self, def: DefId, args: &[Ty], out: &mut Vec<(MethodSig, FnSig)>) {
        let subst = self.subst_for(def, args);
        let sig = self.sig(def);
        for method in sig.methods.values().flatten() {
            if !method.is_abstract || method.receiver.is_none() || self.is_object_method(method) {
                continue;
            }
            let fn_sig = instantiate(method, &subst);
            let seen = out
                .iter()
                .any(|(_, other)| other.name == fn_sig.name && other.params == fn_sig.params);
            if !seen {
                out.push((method.clone(), fn_sig));
            }
        }
        for parent in sig.superclass.iter().chain(&sig.interfaces) {
            if let Ty::Adt { def, args, .. } = parent.subst(&subst) {
                self.collect_abstract_methods(def, &args, out);
            }
        }
    }

    fn is_object_method(&self, method: &MethodSig) -> bool {
        let Some(object) = self.lang_items.get(LangItem::Object) else {
            return false;
        };
        let object = self.sig(object);
        object
            .methods
            .get(&method.name.name)
            .is_some_and(|methods| {
                methods
                    .iter()
                    .any(|other| other.params.len() == method.params.len())
            })
    }
}

fn callee(method: &MethodSig, sig: FnSig, subst: &Subst) -> Callee {
    Callee {
        method: method.clone(),
        params: sig.params,
        ret: sig.ret,
        packs_varargs: false,
        subst: subst.clone(),
    }
}

pub(crate) fn instantiate(method: &MethodSig, subst: &Subst) -> FnSig {
    FnSig {
        name: method.name.name,
        params: method
            .params
            .iter()
            .map(|param| param.subst(subst))
            .collect(),
        ret: method.ret.subst(subst),
        throws: method.throws.iter().map(|ty| ty.subst(subst)).collect(),
    }
}

pub(crate) fn is_closure(expr: &Expr) -> bool {
    matches!(
        expr.kind,
        ExprKind::Lambda { .. } | ExprKind::MethodRef { .. }
    )
}

enum RefReceiver {
    Type(Ty),
    Value(Ty),
}

impl FnCtxt<'_, '_> {
    pub(crate) fn closure_fits(&self, expr: &Expr, expected: &Ty) -> Option<bool> {
        let arity = match &expr.kind {
            ExprKind::Lambda { params, .. } => Some(params.len()),
            ExprKind::MethodRef { .. } => None,
            _ => return None,
        };
        let fits = match expected.non_null() {
            Ty::Unknown => true,
            target => self
                .tcx
                .fn_sig(target)
                .is_some_and(|sig| arity.is_none_or(|arity| arity == sig.params.len())),
        };
        Some(fits)
    }

    pub(crate) fn check_closure(&mut self, expr: &Expr, expected: &Ty) -> Ty {
        let target = expected.non_null();
        let sig = match target {
            Ty::Unknown => None,
            target => {
                let sig = self.tcx.fn_sig(target);
                if sig.is_none() {
                    self.not_functional(expr, expected);
                }
                sig
            }
        };

        let ty = if sig.is_some() {
            expected.clone()
        } else {
            Ty::Unknown
        };
        match &expr.kind {
            ExprKind::Lambda { params, body, .. } => {
                self.check_lambda(expr.span, params, body, sig.as_ref())
            }
            ExprKind::MethodRef { expr: target, name } => {
                self.check_method_ref(target, *name, sig.as_ref(), expected)
            }
            _ => return self.check_expr_coerce(expr, expected),
        }
        self.results.expr_types.insert(expr.span, ty.clone());
        ty
    }

    pub(crate) fn check_closure_args(&mut self, args: &[Expr]) -> Vec<Ty> {
        args.iter()
            .map(|arg| {
                if is_closure(arg) || self.is_diamond(arg) {
                    Ty::Unknown
                } else {
                    self.check_expr(arg)
                }
            })
            .collect()
    }

    pub(crate) fn finish_closure_args(&mut self, args: &[Expr]) {
        for arg in args {
            if self.results.expr_types.contains_key(&arg.span) {
                continue;
            }
            if is_closure(arg) {
                self.check_closure(arg, &Ty::Unknown);
            } else if self.is_diamond(arg) {
                self.check_expr(arg);
            }
        }
    }

    pub(crate) fn is_diamond(&self, expr: &Expr) -> bool {
        let ExprKind::New { ty, .. } = &expr.kind else {
            return false;
        };
        let krelpin_ast::ast::TyKind::Path { args, .. } = &ty.kind else {
            return false;
        };
        args.is_empty()
            && matches!(
                self.tcx.lower_ty(ty),
                Ty::Adt { def, .. } if !self.tcx.sig(def).generics.is_empty()
            )
    }

    fn check_lambda(
        &mut self,
        span: Span,
        params: &[LambdaParam],
        body: &LambdaBody,
        sig: Option<&FnSig>,
    ) {
        let expected_params = match sig {
            Some(sig) if sig.params.len() == params.len() => sig.params.clone(),
            Some(sig) => {
                self.error(
                    span,
                    format!(
                        "this lambda takes {} but `{}` takes {}",
                        plural(params.len(), "parameter"),
                        sig.name,
                        plural(sig.params.len(), "parameter")
                    ),
                );
                vec![Ty::Unknown; params.len()]
            }
            None => vec![Ty::Unknown; params.len()],
        };

        for (param, expected) in params.iter().zip(expected_params) {
            let ty = match &param.ty {
                Some(ty) => {
                    let declared = self.lower_ty(ty);
                    let known = !declared.is_unknown() && !expected.is_unknown();
                    if known && declared != expected {
                        self.mismatch(ty.span, &expected, &declared);
                    }
                    declared
                }
                None => expected,
            };
            self.locals.insert(param.name.span, ty);
        }

        let ret = sig.map_or(Ty::Unknown, |sig| sig.ret.clone());
        let outer_ret = std::mem::replace(&mut self.ret, ret.clone());
        let outer_async = std::mem::replace(&mut self.in_async, false);
        let outer_yields = std::mem::take(&mut self.yields);
        let outer_exceptions =
            std::mem::replace(&mut self.exceptions, ExceptionScope::for_lambda(sig));
        match body {
            LambdaBody::Expr(body) if matches!(ret, Ty::Void | Ty::Unknown) => {
                self.check_expr(body);
            }
            LambdaBody::Expr(body) => {
                self.check_expr_coerce(body, &ret);
            }
            LambdaBody::Block(block) => self.check_lambda_block(block, &ret),
        }
        self.ret = outer_ret;
        self.in_async = outer_async;
        self.yields = outer_yields;
        self.exceptions = outer_exceptions;
    }

    pub(crate) fn probe_lambda(
        &mut self,
        params: &[LambdaParam],
        body: &LambdaBody,
        sig: &FnSig,
    ) -> Option<Ty> {
        if params.len() != sig.params.len() {
            return None;
        }
        for (param, expected) in params.iter().zip(&sig.params) {
            let ty = match &param.ty {
                Some(ty) => self.lower_ty(ty),
                None => expected.clone(),
            };
            self.locals.insert(param.name.span, ty);
        }
        let outer_ret = std::mem::replace(&mut self.ret, Ty::Unknown);
        let outer_async = std::mem::replace(&mut self.in_async, false);
        let outer_yields = std::mem::take(&mut self.yields);
        let probe = FnSig {
            ret: Ty::Unknown,
            ..sig.clone()
        };
        let outer_exceptions = std::mem::replace(
            &mut self.exceptions,
            ExceptionScope::for_lambda(Some(&probe)),
        );
        let ty = match body {
            LambdaBody::Expr(body) => Some(self.check_expr(body)),
            LambdaBody::Block(block) => {
                self.inferred_returns.push(Vec::new());
                self.check_block(block);
                let returns = self.inferred_returns.pop().unwrap_or_default();
                returns
                    .into_iter()
                    .reduce(|a, b| self.tcx.common_ty(&self.param_env, &a, &b).unwrap_or(a))
            }
        };
        self.ret = outer_ret;
        self.in_async = outer_async;
        self.yields = outer_yields;
        self.exceptions = outer_exceptions;
        ty
    }

    fn check_lambda_block(&mut self, block: &Block, ret: &Ty) {
        let diverges = self.check_block(block);
        if !diverges {
            self.report_missing_return(block.span, ret, "lambda");
        }
    }

    fn check_method_ref(&mut self, target: &Expr, name: Ident, sig: Option<&FnSig>, expected: &Ty) {
        let receiver = match self.type_of_name(target) {
            Some(ty) => RefReceiver::Type(ty),
            None => RefReceiver::Value(self.check_expr(target)),
        };
        let Some(sig) = sig else {
            return;
        };
        let (RefReceiver::Type(ty) | RefReceiver::Value(ty)) = &receiver;
        if matches!(ty.strip_refs(), Ty::Unknown) {
            return;
        }

        let Some((ret, callee)) = self.resolve_method_ref(&receiver, name, sig) else {
            let params: Vec<String> = sig.params.iter().map(Ty::to_string).collect();
            self.emit(
                Diagnostic::error(format!("`{ty}::{}` does not match `{expected}`", name.name))
                    .with_span(name.span)
                    .with_note(format!(
                        "`{expected}` needs a method that takes ({}) and returns `{}`",
                        params.join(", "),
                        sig.ret
                    )),
            );
            return;
        };
        if sig.ret != Ty::Void && !self.is_assignable(&ret, &sig.ret) {
            self.mismatch(name.span, &sig.ret, &ret);
        }
        if let Some(callee) = callee {
            self.results.callees.insert(name.span, callee);
        }
    }

    fn resolve_method_ref(
        &self,
        receiver: &RefReceiver,
        name: Ident,
        sig: &FnSig,
    ) -> Option<(Ty, Option<Callee>)> {
        let fits = |params: &[Ty], args: &[Ty]| {
            params.len() == args.len()
                && args
                    .iter()
                    .zip(params)
                    .all(|(arg, param)| self.is_assignable(arg, param))
        };

        if let (RefReceiver::Type(ty @ Ty::Adt { def, args, .. }), kw::New) = (receiver, name.name)
        {
            let subst = self.tcx.subst_for(*def, args);
            let ctors = &self.tcx.sig(*def).ctors;
            if ctors.is_empty() && sig.params.is_empty() {
                return Some((ty.clone(), None));
            }
            return ctors.iter().find_map(|ctor| {
                let ctor_sig = instantiate(ctor, &subst);
                fits(&ctor_sig.params, &sig.params)
                    .then(|| (ty.clone(), Some(callee(ctor, ctor_sig, &subst))))
            });
        }

        let (ty, is_type) = match receiver {
            RefReceiver::Type(ty) => (ty.strip_refs(), true),
            RefReceiver::Value(ty) => (ty.strip_refs(), false),
        };
        self.receiver_adts(ty).into_iter().find_map(|(def, args)| {
            self.tcx
                .methods_named(def, &args, name.name)
                .into_iter()
                .find_map(|(method, subst)| {
                    let method_sig = instantiate(&method, &subst);
                    let matches = match (method.receiver, is_type) {
                        (Some(_), false) | (None, true) => fits(&method_sig.params, &sig.params),
                        (Some(_), true) => sig.params.split_first().is_some_and(|(first, rest)| {
                            self.is_assignable(first.strip_refs(), ty)
                                && fits(&method_sig.params, rest)
                        }),
                        (None, false) => false,
                    };
                    matches.then(|| {
                        (
                            method_sig.ret.clone(),
                            Some(callee(&method, method_sig, &subst)),
                        )
                    })
                })
        })
    }

    fn not_functional(&self, expr: &Expr, expected: &Ty) {
        let what = match expr.kind {
            ExprKind::MethodRef { .. } => "a method reference",
            _ => "a lambda",
        };
        self.emit(
            Diagnostic::error(format!("{what} cannot be used as `{expected}`"))
                .with_span(expr.span)
                .with_note(format!(
                    "`{expected}` is not an interface with exactly one abstract method"
                )),
        );
    }
}

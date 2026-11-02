use krelpin_ast::ast::{self, Item, ItemKind, Member, MemberKind, TyKind};
use krelpin_errors::Diagnostic;
use krelpin_resolve::Res;
use krelpin_span::Span;
use krelpin_span::def_id::DefId;

use crate::check::plural;
use crate::lang_items::LangItem;
use crate::param_env::ParamEnv;
use crate::sig::{GenericParamSig, Tcx};
use crate::ty::Ty;

impl Tcx<'_> {
    pub(crate) fn check_item_wf(&self, def: DefId, item: &Item) {
        let sig = self.sig(def);
        let env = ParamEnv::default().with_generics(&sig.generics);

        let (generics, parents, members): (_, Vec<&ast::Ty>, &[Member]) = match &item.kind {
            ItemKind::Class(class) => (
                Some(&class.generics),
                class.extends.iter().chain(&class.implements).collect(),
                &class.members,
            ),
            ItemKind::Struct(def) => (
                Some(&def.generics),
                def.implements.iter().collect(),
                &def.members,
            ),
            ItemKind::Interface(interface) => (
                Some(&interface.generics),
                interface.extends.iter().collect(),
                &interface.members,
            ),
            ItemKind::Enum(def) => (None, def.implements.iter().collect(), &def.members),
        };

        if let Some(generics) = generics {
            self.check_bounds_wf(&env, generics);
        }
        for parent in parents {
            self.check_wf(&env, parent);
        }
        for member in members {
            self.check_member_wf(&env, member);
        }
    }

    fn check_member_wf(&self, env: &ParamEnv, member: &Member) {
        match &member.kind {
            MemberKind::Field(field) => self.check_wf(env, &field.ty),
            MemberKind::Constructor(ctor) => {
                for param in &ctor.params {
                    self.check_wf(env, &param.ty);
                }
                for ty in &ctor.throws {
                    self.check_wf(env, ty);
                }
                self.check_throws_clause(env, &ctor.throws);
            }
            MemberKind::Method(method) => {
                let env = env
                    .clone()
                    .with_generics(&self.generic_params(&method.generics));
                self.check_bounds_wf(&env, &method.generics);
                if let ast::FnRetTy::Ty(ty) = &method.ret {
                    self.check_wf(&env, ty);
                }
                for param in &method.params {
                    self.check_wf(&env, &param.ty);
                }
                for ty in &method.throws {
                    self.check_wf(&env, ty);
                }
                self.check_throws_clause(&env, &method.throws);
            }
        }
    }

    fn check_bounds_wf(&self, env: &ParamEnv, generics: &ast::Generics) {
        for param in &generics.params {
            if let ast::GenericParamKind::Type { bounds } = &param.kind {
                for bound in bounds {
                    self.check_wf(env, bound);
                }
            }
        }
    }

    pub(crate) fn check_wf(&self, env: &ParamEnv, ty: &ast::Ty) {
        match &ty.kind {
            TyKind::Primitive(_) => {}
            TyKind::Path { path, args } => {
                for arg in args {
                    self.check_wf(env, arg);
                }
                let [segment] = path.segments.as_slice() else {
                    return;
                };
                match self.resolutions.get(segment.span) {
                    Some(Res::Item(def)) => self.check_generic_args(env, def, args, ty.span),
                    Some(Res::TypeParam(_)) if !args.is_empty() => {
                        self.dcx.emit(
                            Diagnostic::error(format!(
                                "type parameter `{}` does not take type arguments",
                                segment.name
                            ))
                            .with_span(ty.span),
                        );
                    }
                    _ => {}
                }
            }
            TyKind::Ref { ty, .. } | TyKind::Nullable(ty) | TyKind::Array(ty) => {
                self.check_wf(env, ty);
            }
        }
    }

    fn check_generic_args(&self, env: &ParamEnv, def: DefId, args: &[ast::Ty], span: Span) {
        let generics = &self.sig(def).generics;
        if args.is_empty() {
            return;
        }
        if args.len() != generics.len() {
            let supplied = if args.len() == 1 { "was" } else { "were" };
            self.dcx.emit(
                Diagnostic::error(format!(
                    "`{}` takes {} but {} {supplied} supplied",
                    self.sig(def).name.name,
                    plural(generics.len(), "type argument"),
                    args.len()
                ))
                .with_span(span),
            );
            return;
        }

        let lowered: Vec<Ty> = args.iter().map(|arg| self.lower_ty(arg)).collect();
        let subst = self.subst_for(def, &lowered);
        for ((param, arg), ast_arg) in generics.iter().zip(&lowered).zip(args) {
            if let Ty::Primitive(primitive) = arg {
                let boxed = self.lang_ty(LangItem::boxed(*primitive));
                self.dcx.emit(
                    Diagnostic::error("type arguments cannot be primitive types")
                        .with_span(ast_arg.span)
                        .with_help(format!("use `{boxed}` instead")),
                );
                continue;
            }
            self.check_bound(env, param, arg, &subst, ast_arg.span);
        }
    }

    pub(crate) fn check_bound(
        &self,
        env: &ParamEnv,
        param: &GenericParamSig,
        arg: &Ty,
        subst: &crate::ty::Subst,
        span: Span,
    ) {
        for bound in &param.bounds {
            let bound = bound.subst(subst);
            if !self.is_assignable(env, arg, &bound) {
                self.dcx.emit(
                    Diagnostic::error(format!(
                        "type `{arg}` does not satisfy the bound of `{}`",
                        param.name
                    ))
                    .with_span(span)
                    .with_label(format!("`{arg}` is not a `{bound}`")),
                );
            }
        }
    }
}

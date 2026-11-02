use krelpin_ast::ast::{self, Catch, Expr};
use krelpin_errors::Diagnostic;

use crate::check::FnCtxt;
use crate::lang_items::LangItem;
use crate::param_env::ParamEnv;
use crate::sig::Tcx;
use crate::ty::Ty;

const ONLY_THROWABLE: &str = "only subclasses of `Throwable` can be thrown and caught";

impl Tcx<'_> {
    pub(crate) fn is_throwable(&self, env: &ParamEnv, ty: &Ty) -> bool {
        match ty {
            Ty::Unknown | Ty::External { .. } => true,
            ty => self.is_assignable(env, ty, &self.lang_ty(LangItem::Throwable)),
        }
    }

    pub(crate) fn check_throws_clause(&self, env: &ParamEnv, tys: &[ast::Ty]) {
        for ast_ty in tys {
            let ty = self.lower_ty(ast_ty);
            if !self.is_throwable(env, &ty) {
                self.dcx.emit(
                    Diagnostic::error(format!("`{ty}` is not a `Throwable`"))
                        .with_span(ast_ty.span)
                        .with_note(ONLY_THROWABLE),
                );
            }
        }
    }

    fn superclass_chain(&self, ty: &Ty) -> Vec<Ty> {
        let mut chain = vec![ty.clone()];
        while let Some(Ty::Adt { def, .. }) = chain.last() {
            match &self.sig(*def).superclass {
                Some(parent) => chain.push(parent.clone()),
                None => break,
            }
        }
        chain
    }
}

impl FnCtxt<'_, '_> {
    pub(crate) fn check_throw(&mut self, value: &Expr) {
        let ty = self.check_expr(value);
        if !self.tcx.is_throwable(&self.param_env, &ty) {
            self.emit(
                Diagnostic::error(format!("cannot throw a value of type `{ty}`"))
                    .with_span(value.span)
                    .with_note(ONLY_THROWABLE),
            );
        }
    }

    pub(crate) fn check_catch(&mut self, catch: &Catch, caught: &mut Vec<Ty>) -> Ty {
        let mut alternatives: Vec<Ty> = Vec::new();
        for ast_ty in &catch.tys {
            let ty = self.lower_ty(ast_ty);
            if ty.references_unknown() || matches!(ty, Ty::External { .. }) {
                continue;
            }
            if !self.tcx.is_throwable(&self.param_env, &ty) {
                self.emit(
                    Diagnostic::error(format!("`{ty}` cannot be caught"))
                        .with_span(ast_ty.span)
                        .with_note(ONLY_THROWABLE),
                );
                continue;
            }

            let earlier = caught.iter().find(|earlier| self.is_subtype(&ty, earlier));
            let overlapping = alternatives
                .iter()
                .find(|other| self.is_subtype(&ty, other) || self.is_subtype(other, &ty));
            if let Some(earlier) = earlier {
                self.emit(
                    Diagnostic::error(format!("`{ty}` has already been caught"))
                        .with_span(ast_ty.span)
                        .with_label(format!("already caught by the `catch` for `{earlier}`")),
                );
            } else if let Some(other) = overlapping {
                self.emit(
                    Diagnostic::error(format!("`{ty}` and `{other}` overlap in this `catch`"))
                        .with_span(ast_ty.span)
                        .with_help("catch only the more general one"),
                );
            }
            alternatives.push(ty);
        }

        caught.extend(alternatives.iter().cloned());
        self.common_superclass(&alternatives)
    }

    fn common_superclass(&self, tys: &[Ty]) -> Ty {
        let Some((first, rest)) = tys.split_first() else {
            return Ty::Unknown;
        };
        self.tcx
            .superclass_chain(first)
            .into_iter()
            .find(|candidate| rest.iter().all(|ty| self.is_subtype(ty, candidate)))
            .unwrap_or_else(|| self.tcx.lang_ty(LangItem::Throwable))
    }

    fn is_subtype(&self, from: &Ty, to: &Ty) -> bool {
        self.tcx.is_assignable(&self.param_env, from, to)
    }
}

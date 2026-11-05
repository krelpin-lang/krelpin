use krelpin_ast::ast::{self, Catch, Expr};
use krelpin_errors::Diagnostic;
use krelpin_span::{Span, Symbol};

use crate::check::FnCtxt;
use crate::closure::FnSig;
use crate::lang_items::LangItem;
use crate::param_env::ParamEnv;
use crate::sig::Tcx;
use crate::ty::Ty;

const ONLY_THROWABLE: &str = "only subclasses of `Throwable` can be thrown and caught";

#[derive(Debug, Default)]
pub(crate) struct ExceptionScope {
    declared: Vec<Ty>,
    lambda: Option<Symbol>,
    frames: Vec<TryFrame>,
}

#[derive(Debug, Default)]
pub(crate) struct TryFrame {
    catches: Vec<Ty>,
    thrown: Vec<Ty>,
    calls_unknown_code: bool,
}

impl ExceptionScope {
    pub(crate) fn declared(declared: Vec<Ty>) -> ExceptionScope {
        ExceptionScope {
            declared,
            ..ExceptionScope::default()
        }
    }

    pub(crate) fn for_lambda(sig: Option<&FnSig>) -> ExceptionScope {
        ExceptionScope {
            declared: sig.map_or_else(Vec::new, |sig| sig.throws.clone()),
            lambda: sig.map(|sig| sig.name),
            frames: Vec::new(),
        }
    }
}

impl Tcx<'_> {
    pub(crate) fn is_throwable(&self, env: &ParamEnv, ty: &Ty) -> bool {
        match ty {
            Ty::Unknown => true,
            ty => self.is_assignable(env, ty, &self.lang_ty(LangItem::Throwable)),
        }
    }

    pub(crate) fn is_checked(&self, env: &ParamEnv, ty: &Ty) -> bool {
        let unchecked = |item| self.is_assignable(env, ty, &self.lang_ty(item));
        !matches!(ty, Ty::Unknown | Ty::Null)
            && self.is_throwable(env, ty)
            && !unchecked(LangItem::RuntimeException)
            && !unchecked(LangItem::Error)
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
        if self.tcx.is_throwable(&self.param_env, &ty) {
            self.raise(ty.non_null().clone(), value.span);
        } else {
            self.emit(
                Diagnostic::error(format!("cannot throw a value of type `{ty}`"))
                    .with_span(value.span)
                    .with_note(ONLY_THROWABLE),
            );
        }
    }

    pub(crate) fn raise(&mut self, ty: Ty, span: Span) {
        let mut handled = false;
        for index in (0..self.exceptions.frames.len()).rev() {
            let catches = &self.exceptions.frames[index].catches;
            handled = catches.iter().any(|catch| self.is_subtype(&ty, catch));
            self.exceptions.frames[index].thrown.push(ty.clone());
            if handled {
                break;
            }
        }
        let declared = self
            .exceptions
            .declared
            .iter()
            .any(|declared| self.is_subtype(&ty, declared));
        if handled || declared || !self.tcx.is_checked(&self.param_env, &ty) {
            return;
        }

        let help = match self.exceptions.lambda {
            Some(method) => {
                format!("catch it inside the lambda, because `{method}` cannot throw it")
            }
            None => format!("catch it with `try`, or add `throws {ty}` to the method"),
        };
        self.emit(
            Diagnostic::error(format!("unreported exception `{ty}`"))
                .with_span(span)
                .with_note("checked exceptions must be caught or declared with `throws`")
                .with_help(help),
        );
    }

    pub(crate) fn calls_unknown_code(&mut self) {
        for frame in &mut self.exceptions.frames {
            frame.calls_unknown_code = true;
        }
    }

    pub(crate) fn enter_try(&mut self, catches: &[Catch]) {
        let catches = catches
            .iter()
            .flat_map(|catch| &catch.tys)
            .map(|ty| self.tcx.lower_ty(ty))
            .collect();
        self.exceptions.frames.push(TryFrame {
            catches,
            ..TryFrame::default()
        });
    }

    pub(crate) fn exit_try(&mut self) -> TryFrame {
        self.exceptions.frames.pop().unwrap_or_default()
    }

    pub(crate) fn check_catch(
        &mut self,
        catch: &Catch,
        body: &TryFrame,
        caught: &mut Vec<Ty>,
    ) -> Ty {
        let mut alternatives: Vec<Ty> = Vec::new();
        for ast_ty in &catch.tys {
            let ty = self.lower_ty(ast_ty);
            if ty.references_unknown() {
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

            self.check_catch_reachable(&ty, ast_ty.span, body);
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

    fn check_catch_reachable(&self, ty: &Ty, span: Span, body: &TryFrame) {
        let catches_anything = [LangItem::Exception, LangItem::Throwable]
            .into_iter()
            .any(|item| self.tcx.is_lang_ty(ty, item));
        if body.calls_unknown_code || catches_anything || !self.tcx.is_checked(&self.param_env, ty)
        {
            return;
        }
        let thrown = body
            .thrown
            .iter()
            .any(|thrown| self.is_subtype(thrown, ty) || self.is_subtype(ty, thrown));
        if !thrown {
            self.emit(
                Diagnostic::error(format!("`{ty}` is never thrown in the body of this `try`"))
                    .with_span(span)
                    .with_help("remove the `catch`, or catch an exception the body can throw"),
            );
        }
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

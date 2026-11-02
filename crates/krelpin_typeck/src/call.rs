use krelpin_ast::ast::{Expr, ExprKind, Ident, PrimitiveTy, UnOp};
use krelpin_ast::token::LitKind;
use krelpin_errors::Diagnostic;
use krelpin_span::Symbol;

use crate::check::{FnCtxt, plural};
use crate::coerce::Coercion;
use crate::lang_items::LangItem;
use crate::sig::MethodSig;
use crate::ty::{Subst, Ty};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Strict,
    Loose,
    Variadic,
}

impl Phase {
    const ALL: [Phase; 3] = [Phase::Strict, Phase::Loose, Phase::Variadic];

    fn coercion(self) -> Coercion {
        match self {
            Phase::Strict => Coercion::Subtype,
            Phase::Loose | Phase::Variadic => Coercion::Boxing,
        }
    }
}

struct Candidate {
    method: MethodSig,
    subst: Subst,
    params: Vec<Ty>,
    ret: Ty,
}

impl Candidate {
    fn accepts_arity(&self, args: usize, phase: Phase) -> bool {
        match phase {
            Phase::Strict | Phase::Loose => args == self.params.len(),
            Phase::Variadic => self.method.variadic && args + 1 >= self.params.len(),
        }
    }

    fn param(&self, index: usize, phase: Phase) -> Ty {
        let last = self.params.len().saturating_sub(1);
        match (self.params.last(), phase) {
            (Some(Ty::Array(elem)), Phase::Variadic) if index >= last => elem.as_ref().clone(),
            _ => self.params.get(index).cloned().unwrap_or(Ty::Unknown),
        }
    }
}

impl FnCtxt<'_, '_> {
    pub(crate) fn accepts(
        &self,
        expr: &Expr,
        actual: &Ty,
        expected: &Ty,
        coercion: Coercion,
    ) -> bool {
        let narrows_literal = coercion == Coercion::Boxing
            && is_int_literal(expr)
            && matches!(
                expected.non_null(),
                Ty::Primitive(PrimitiveTy::Byte | PrimitiveTy::Short | PrimitiveTy::Char)
            );
        narrows_literal
            || self
                .tcx
                .coerces(&self.param_env, actual, expected, coercion)
    }

    pub(crate) fn call(
        &mut self,
        candidates: Vec<(MethodSig, Subst)>,
        args: &[Expr],
        arg_tys: &[Ty],
        name: Ident,
        what: &str,
    ) -> Ty {
        let candidates: Vec<Candidate> = candidates
            .into_iter()
            .map(|(method, subst)| self.instantiate(method, subst, arg_tys))
            .collect();
        let arity_ok: Vec<&Candidate> = candidates
            .iter()
            .filter(|candidate| {
                Phase::ALL
                    .iter()
                    .any(|&phase| candidate.accepts_arity(args.len(), phase))
            })
            .collect();
        if arity_ok.is_empty() {
            self.report_arity(&candidates, args.len(), name, what);
            return Ty::Unknown;
        }

        let applicable = Phase::ALL.iter().find_map(|&phase| {
            let applicable: Vec<&Candidate> = arity_ok
                .iter()
                .copied()
                .filter(|candidate| self.is_applicable(candidate, args, arg_tys, phase))
                .collect();
            (!applicable.is_empty()).then_some((phase, applicable))
        });
        let Some((phase, applicable)) = applicable else {
            if let [candidate] = arity_ok.as_slice() {
                self.report_arg_mismatches(candidate, args, arg_tys);
                return candidate.ret.clone();
            }
            let found: Vec<String> = arg_tys.iter().map(Ty::to_string).collect();
            self.error(
                name.span,
                format!(
                    "no overload of `{}` accepts arguments of type ({})",
                    name.name,
                    found.join(", ")
                ),
            );
            return Ty::Unknown;
        };

        let Some(chosen) = self.most_specific(&applicable, args.len(), phase) else {
            if !arg_tys.iter().any(Ty::references_unknown) {
                self.report_ambiguity(&applicable, name);
            }
            return Ty::Unknown;
        };
        self.check_inferred_bounds(chosen, name);
        self.check_vis(chosen.method.vis, chosen.method.owner, false, what, name);
        chosen.ret.clone()
    }

    fn instantiate(&self, method: MethodSig, mut subst: Subst, arg_tys: &[Ty]) -> Candidate {
        if !method.generics.is_empty() {
            let vars: Vec<Symbol> = method.generics.iter().map(|param| param.name).collect();
            let mut inferred = Subst::new();
            for (index, arg) in arg_tys.iter().enumerate() {
                if let Some(param) = param_for_inference(&method, index, arg_tys.len(), arg) {
                    self.infer(&param.subst(&subst), arg, &vars, &mut inferred);
                }
            }
            for var in vars {
                let ty = inferred.remove(&var).unwrap_or(Ty::Unknown);
                subst.insert(var, ty);
            }
        }

        Candidate {
            params: method
                .params
                .iter()
                .map(|param| param.subst(&subst))
                .collect(),
            ret: method.ret.subst(&subst),
            method,
            subst,
        }
    }

    fn infer(&self, param: &Ty, arg: &Ty, vars: &[Symbol], inferred: &mut Subst) {
        match (param, arg) {
            (_, Ty::Unknown | Ty::Null) => {}
            (Ty::Param(name), _) if vars.contains(name) => {
                let arg = match arg {
                    Ty::Primitive(primitive) => self.tcx.lang_ty(LangItem::boxed(*primitive)),
                    arg => arg.clone(),
                };
                let merged = match inferred.get(name) {
                    None => Some(arg),
                    Some(previous) => self.tcx.common_ty(&self.param_env, previous, &arg),
                };
                if let Some(merged) = merged {
                    inferred.insert(*name, merged);
                }
            }
            (Ty::Nullable(param), Ty::Nullable(arg))
            | (Ty::Ref { ty: param, .. }, Ty::Ref { ty: arg, .. })
            | (Ty::Array(param), Ty::Array(arg)) => self.infer(param, arg, vars, inferred),
            (Ty::Nullable(param), arg) => self.infer(param, arg, vars, inferred),
            (
                Ty::Adt {
                    def, args: params, ..
                },
                Ty::Adt {
                    def: arg_def,
                    args: arg_args,
                    ..
                },
            ) => {
                if let Some(args) = self.tcx.upcast(*arg_def, arg_args, *def) {
                    for (param, arg) in params.iter().zip(&args) {
                        self.infer(param, arg, vars, inferred);
                    }
                }
            }
            _ => {}
        }
    }

    fn is_applicable(
        &self,
        candidate: &Candidate,
        args: &[Expr],
        arg_tys: &[Ty],
        phase: Phase,
    ) -> bool {
        candidate.accepts_arity(args.len(), phase)
            && args
                .iter()
                .zip(arg_tys)
                .enumerate()
                .all(|(index, (arg, ty))| {
                    self.accepts(arg, ty, &candidate.param(index, phase), phase.coercion())
                })
    }

    fn most_specific<'c>(
        &self,
        applicable: &[&'c Candidate],
        arg_count: usize,
        phase: Phase,
    ) -> Option<&'c Candidate> {
        let more_specific = |a: &Candidate, b: &Candidate| {
            (0..arg_count).all(|index| {
                self.tcx.is_subtype(
                    &self.param_env,
                    &a.param(index, phase),
                    &b.param(index, phase),
                )
            })
        };
        applicable
            .iter()
            .copied()
            .find(|a| applicable.iter().all(|b| more_specific(a, b)))
    }

    fn check_inferred_bounds(&self, candidate: &Candidate, name: Ident) {
        for param in &candidate.method.generics {
            match candidate.subst.get(&param.name) {
                Some(Ty::Unknown) | None => {}
                Some(arg) => {
                    self.tcx
                        .check_bound(&self.param_env, param, arg, &candidate.subst, name.span)
                }
            }
        }
    }

    fn report_arity(&self, candidates: &[Candidate], arg_count: usize, name: Ident, what: &str) {
        let message = match candidates {
            [candidate] => format!(
                "this {what} takes {} but {} {} supplied",
                plural_args(candidate.params.len(), candidate.method.variadic),
                plural(arg_count, "argument"),
                if arg_count == 1 { "was" } else { "were" },
            ),
            _ => format!(
                "no overload of `{}` takes {}",
                name.name,
                plural(arg_count, "argument")
            ),
        };
        self.error(name.span, message);
    }

    fn report_arg_mismatches(&self, candidate: &Candidate, args: &[Expr], arg_tys: &[Ty]) {
        for (index, (arg, ty)) in args.iter().zip(arg_tys).enumerate() {
            let phases: &[Phase] = if candidate.accepts_arity(args.len(), Phase::Loose) {
                &[Phase::Loose, Phase::Variadic]
            } else {
                &[Phase::Variadic]
            };
            let expected: Vec<Ty> = phases
                .iter()
                .filter(|&&phase| candidate.accepts_arity(args.len(), phase))
                .map(|&phase| candidate.param(index, phase))
                .collect();
            let fits = expected
                .iter()
                .any(|expected| self.accepts(arg, ty, expected, Coercion::Boxing));
            if let (false, Some(expected)) = (fits, expected.last()) {
                self.mismatch(arg.span, expected, ty);
            }
        }
    }

    fn report_ambiguity(&self, applicable: &[&Candidate], name: Ident) {
        let signatures: Vec<String> = applicable
            .iter()
            .map(|candidate| {
                let params: Vec<String> = candidate.params.iter().map(Ty::to_string).collect();
                format!(
                    "`{}.{}({})`",
                    self.type_name(candidate.method.owner),
                    name.name,
                    params.join(", ")
                )
            })
            .collect();
        self.emit(
            Diagnostic::error(format!("call to `{}` is ambiguous", name.name))
                .with_span(name.span)
                .with_note(format!("it could call {}", signatures.join(" or ")))
                .with_help("cast the arguments to the types of the method you want to call"),
        );
    }
}

fn param_for_inference(method: &MethodSig, index: usize, arg_count: usize, arg: &Ty) -> Option<Ty> {
    let last = method.params.len().checked_sub(1)?;
    match &method.params[last] {
        Ty::Array(elem) if method.variadic && index >= last => {
            let passes_array = index == last && arg_count == method.params.len();
            if passes_array && matches!(arg, Ty::Array(_)) {
                Some(method.params[last].clone())
            } else {
                Some(elem.as_ref().clone())
            }
        }
        _ => method.params.get(index).cloned(),
    }
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

fn plural_args(count: usize, variadic: bool) -> String {
    if variadic {
        format!("at least {}", plural(count.saturating_sub(1), "argument"))
    } else {
        plural(count, "argument")
    }
}

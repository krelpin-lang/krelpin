use krelpin_ast::ast::{Expr, Ident};
use krelpin_errors::Diagnostic;
use krelpin_span::Symbol;

use crate::check::{FnCtxt, plural};
use crate::closure::is_closure;
use crate::sig::MethodSig;
use crate::ty::{Subst, Ty};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Fixed,
    Variadic,
}

impl Phase {
    const ALL: [Phase; 2] = [Phase::Fixed, Phase::Variadic];
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
            Phase::Fixed => args == self.params.len(),
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
    pub(crate) fn accepts(&self, expr: &Expr, actual: &Ty, expected: &Ty) -> bool {
        match self.closure_fits(expr, expected) {
            Some(fits) => fits,
            None => self.is_assignable(actual, expected),
        }
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
            let unknown_arg = args
                .iter()
                .zip(arg_tys)
                .any(|(arg, ty)| !is_closure(arg) && ty.references_unknown());
            if !unknown_arg {
                self.report_ambiguity(&applicable, name);
            }
            return Ty::Unknown;
        };
        self.check_call_bounds(chosen, name);
        self.check_vis(chosen.method.vis, chosen.method.owner, false, what, name);
        for thrown in &chosen.method.throws {
            self.raise(thrown.subst(&chosen.subst), name.span);
        }
        for (index, arg) in args.iter().enumerate() {
            if is_closure(arg) {
                self.check_closure(arg, &chosen.param(index, phase));
            }
        }
        chosen.ret.clone()
    }

    fn instantiate(&self, method: MethodSig, mut subst: Subst, arg_tys: &[Ty]) -> Candidate {
        if !method.generics.is_empty() {
            let vars: Vec<Symbol> = method.generics.iter().map(|param| param.name).collect();
            for var in &vars {
                subst.remove(var);
            }
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
                let merged = match inferred.get(name) {
                    None => Some(arg.clone()),
                    Some(previous) => self.tcx.common_ty(&self.param_env, previous, arg),
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
                .all(|(index, (arg, ty))| self.accepts(arg, ty, &candidate.param(index, phase)))
    }

    fn most_specific<'c>(
        &self,
        applicable: &[&'c Candidate],
        arg_count: usize,
        phase: Phase,
    ) -> Option<&'c Candidate> {
        let more_specific = |a: &Candidate, b: &Candidate| {
            (0..arg_count).all(|index| {
                self.tcx.is_assignable(
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

    fn check_call_bounds(&self, candidate: &Candidate, name: Ident) {
        let method = &candidate.method;
        for param in method.generics.iter().chain(&method.where_bounds) {
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

    fn report_arg_mismatches(&mut self, candidate: &Candidate, args: &[Expr], arg_tys: &[Ty]) {
        for (index, (arg, ty)) in args.iter().zip(arg_tys).enumerate() {
            let phases: &[Phase] = if candidate.accepts_arity(args.len(), Phase::Fixed) {
                &[Phase::Fixed, Phase::Variadic]
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
                .any(|expected| self.accepts(arg, ty, expected));
            match (fits, expected.last()) {
                (false, Some(expected)) if is_closure(arg) => {
                    self.check_closure(arg, expected);
                }
                (false, Some(expected)) => self.mismatch(arg.span, expected, ty),
                _ => {}
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

fn plural_args(count: usize, variadic: bool) -> String {
    if variadic {
        format!("at least {}", plural(count.saturating_sub(1), "argument"))
    } else {
        plural(count, "argument")
    }
}

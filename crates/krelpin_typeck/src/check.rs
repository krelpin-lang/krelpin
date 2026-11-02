use std::collections::{HashMap, HashSet};

use krelpin_ast::ast::{
    CompilationUnit, FieldStorage, FnRetTy, ItemKind, Member, MemberKind, Param,
};
use krelpin_errors::Diagnostic;
use krelpin_span::def_id::DefId;
use krelpin_span::{Span, Symbol};

use crate::TypeckResults;
use crate::param_env::ParamEnv;
use crate::sig::Tcx;
use crate::ty::Ty;

pub(crate) struct FnCtxt<'a, 'tcx> {
    pub(crate) tcx: &'a Tcx<'tcx>,
    pub(crate) owner: DefId,
    pub(crate) self_ty: Ty,
    pub(crate) param_env: ParamEnv,
    pub(crate) ret: Ty,
    pub(crate) locals: HashMap<Span, Ty>,
    pub(crate) narrowed: Vec<HashMap<Span, Ty>>,
    pub(crate) yields: Vec<Vec<Ty>>,
    pub(crate) results: HashMap<Span, Ty>,
}

impl Tcx<'_> {
    pub(crate) fn break_inheritance_cycles(&mut self) {
        for def in (0..self.types.len()).map(DefId::from_index) {
            if !self.reaches(def, def) {
                continue;
            }
            let name = self.sig(def).name;
            self.dcx.emit(
                Diagnostic::error(format!(
                    "cyclic inheritance: `{}` inherits from itself",
                    name.name
                ))
                .with_span(name.span)
                .with_note("a type cannot be its own ancestor"),
            );
            let sig = &mut self.types[def.index()];
            sig.superclass = None;
            sig.interfaces
                .retain(|parent| !matches!(parent, Ty::Adt { .. }));
        }
    }

    fn reaches(&self, from: DefId, target: DefId) -> bool {
        let mut stack = vec![from];
        let mut seen = HashSet::new();
        while let Some(def) = stack.pop() {
            let sig = self.sig(def);
            for parent in sig.superclass.iter().chain(sig.interfaces.iter()) {
                if let Ty::Adt { def: parent, .. } = parent {
                    if *parent == target {
                        return true;
                    }
                    if seen.insert(*parent) {
                        stack.push(*parent);
                    }
                }
            }
        }
        false
    }
}

pub(crate) fn check_units(tcx: &Tcx<'_>, units: &[CompilationUnit]) -> TypeckResults {
    let mut results = TypeckResults::default();

    for (type_def, item) in tcx.items.items(units) {
        let owner = type_def.def;
        tcx.check_item_wf(owner, item);
        tcx.check_inheritance(owner, item);
        let members: &[Member] = match &item.kind {
            ItemKind::Class(class) => &class.members,
            ItemKind::Struct(def) => &def.members,
            ItemKind::Interface(interface) => &interface.members,
            ItemKind::Enum(def) => {
                for constant in &def.constants {
                    let mut fcx = FnCtxt::new(tcx, owner, Ty::Void);
                    let arg_tys = fcx.check_closure_args(&constant.args);
                    fcx.check_ctor_call(owner, &[], &constant.args, &arg_tys, constant.name);
                    fcx.finish_closure_args(&constant.args);
                    results.expr_types.extend(fcx.results);
                }
                &def.members
            }
        };

        for member in members {
            let fcx = check_member(tcx, owner, member);
            results.expr_types.extend(fcx.results);
        }
    }

    results
}

fn check_member<'a, 'tcx>(tcx: &'a Tcx<'tcx>, owner: DefId, member: &Member) -> FnCtxt<'a, 'tcx> {
    match &member.kind {
        MemberKind::Field(field) => {
            let mut fcx = FnCtxt::new(tcx, owner, Ty::Void);
            if let Some(init) = &field.init {
                let ty = tcx
                    .sig(owner)
                    .fields
                    .get(&field.name.name)
                    .map(|f| f.ty.clone());
                let expected = ty.unwrap_or(Ty::Unknown);
                if matches!(
                    field.storage,
                    FieldStorage::Shared(_) | FieldStorage::Const(_)
                ) {
                    fcx.self_ty = Ty::Unknown;
                }
                fcx.check_expr_coerce(init, &expected);
            }
            fcx
        }
        MemberKind::Constructor(ctor) => {
            let mut fcx = FnCtxt::new(tcx, owner, Ty::Void);
            fcx.declare_params(&ctor.params);
            fcx.check_block(&ctor.body);
            fcx
        }
        MemberKind::Method(method) => {
            let ret = match &method.ret {
                FnRetTy::Void(_) => Ty::Void,
                FnRetTy::Ty(ty) => tcx.lower_ty(ty),
            };
            let mut fcx = FnCtxt::new(tcx, owner, ret.clone());
            fcx.param_env = fcx
                .param_env
                .with_generics(&tcx.generic_params(&method.generics));
            if method.receiver.is_none() {
                fcx.self_ty = Ty::Unknown;
            }
            fcx.declare_params(&method.params);
            if let Some(body) = &method.body {
                let diverges = fcx.check_block(body);
                if !diverges {
                    fcx.report_missing_return(body.span, &ret, "method");
                }
            }
            fcx
        }
    }
}

impl<'a, 'tcx> FnCtxt<'a, 'tcx> {
    pub(crate) fn new(tcx: &'a Tcx<'tcx>, owner: DefId, ret: Ty) -> Self {
        FnCtxt {
            tcx,
            owner,
            self_ty: tcx.self_ty(owner),
            param_env: ParamEnv::default().with_generics(&tcx.sig(owner).generics),
            ret,
            locals: HashMap::new(),
            narrowed: Vec::new(),
            yields: Vec::new(),
            results: HashMap::new(),
        }
    }

    fn declare_params(&mut self, params: &[Param]) {
        for param in params {
            let ty = self.tcx.lower_ty(&param.ty);
            let ty = if param.variadic {
                Ty::Array(Box::new(ty))
            } else {
                ty
            };
            self.locals.insert(param.name.span, ty);
        }
    }

    pub(crate) fn report_missing_return(&self, body: Span, ret: &Ty, what: &str) {
        if matches!(ret, Ty::Void | Ty::Unknown) {
            return;
        }
        let end = body.hi;
        self.emit(
            Diagnostic::error("missing return statement")
                .with_span(Span::new(end - 1, end))
                .with_label(format!("this {what} must return a `{ret}`"))
                .with_help("add a `return` statement, or `throw` an exception"),
        );
    }

    pub(crate) fn lower_ty(&self, ty: &krelpin_ast::ast::Ty) -> Ty {
        self.tcx.check_wf(&self.param_env, ty);
        self.tcx.lower_ty(ty)
    }

    pub(crate) fn is_assignable(&self, from: &Ty, to: &Ty) -> bool {
        self.tcx.is_assignable(&self.param_env, from, to)
    }

    pub(crate) fn local_ty(&self, decl: Span) -> Ty {
        self.narrowed
            .iter()
            .rev()
            .find_map(|scope| scope.get(&decl))
            .or_else(|| self.locals.get(&decl))
            .cloned()
            .unwrap_or(Ty::Unknown)
    }

    pub(crate) fn type_name(&self, def: DefId) -> Symbol {
        self.tcx.sig(def).name.name
    }

    pub(crate) fn emit(&self, diagnostic: Diagnostic) {
        self.tcx.dcx.emit(diagnostic);
    }

    pub(crate) fn error(&self, span: Span, message: impl Into<String>) {
        self.emit(Diagnostic::error(message).with_span(span));
    }

    pub(crate) fn mismatch(&self, span: Span, expected: &Ty, found: &Ty) {
        let diagnostic = Diagnostic::error("mismatched types")
            .with_span(span)
            .with_label(format!("expected `{expected}`, found `{found}`"));

        let diagnostic = match (expected, found) {
            (_, Ty::Null) => diagnostic.with_help(format!(
                "`{expected}` can never be `null`; use `{expected}?` to allow it"
            )),
            (expected, Ty::Nullable(inner)) if self.is_assignable(inner, expected) => diagnostic
                .with_help(
                    "this value may be `null`; use `?:` to give a default, \
                     or check `!= null` first",
                ),
            (Ty::Ref { ty, .. }, found) if self.is_assignable(found, ty) => {
                diagnostic.with_help("borrow the value with `&`")
            }
            _ => diagnostic,
        };
        self.emit(diagnostic);
    }
}

pub(crate) fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

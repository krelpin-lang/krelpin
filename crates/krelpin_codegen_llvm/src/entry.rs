use krelpin_errors::Diagnostic;
use krelpin_mir::{BodyId, BodySource, Mir};
use krelpin_span::Symbol;
use krelpin_span::def_id::DefId;
use krelpin_typeck::{MethodSig, Tcx, Ty};

pub fn find_entry(
    mir: &Mir,
    tcx: &Tcx<'_>,
    is_user: impl Fn(DefId) -> bool,
) -> Result<BodyId, Box<Diagnostic>> {
    let main = Symbol::intern("main");
    let candidates: Vec<BodyId> = mir
        .bodies
        .iter()
        .enumerate()
        .filter(|(_, body)| {
            body.source == BodySource::Method && body.name == main && is_user(body.owner)
        })
        .map(|(index, _)| BodyId(index as u32))
        .filter(|&id| {
            let body = mir.body(id);
            method_of(tcx, body.owner, body.member).is_some_and(|sig| is_entry(tcx, sig))
        })
        .collect();
    match candidates.as_slice() {
        [entry] => Ok(*entry),
        [] => Err(Box::new(
            Diagnostic::error("this program has no `main` method").with_help(
                "add `static void main()` or `static void main(String[] args)` to a class",
            ),
        )),
        [first, second, ..] => Err(Box::new(
            Diagnostic::error("this program has more than one `main` method")
                .with_span(mir.body(*second).span)
                .with_secondary(mir.body(*first).span, "the first `main` is here")
                .with_help("keep a single `static void main()`"),
        )),
    }
}

pub(crate) fn method_of<'t>(
    tcx: &'t Tcx<'_>,
    owner: DefId,
    member: Option<u32>,
) -> Option<&'t MethodSig> {
    let member = member?;
    let sig = tcx.sig(owner);
    sig.methods
        .values()
        .flatten()
        .chain(&sig.ctors)
        .find(|method| method.member == Some(member))
}

fn is_entry(tcx: &Tcx<'_>, sig: &MethodSig) -> bool {
    if sig.receiver.is_some() || sig.ret != Ty::Void || !sig.generics.is_empty() {
        return false;
    }
    match sig.params.as_slice() {
        [] => true,
        [Ty::Array(elem)] => tcx.is_string(elem),
        _ => false,
    }
}

pub fn check_test(mir: &Mir, tcx: &Tcx<'_>, body: BodyId) -> Result<(), Box<Diagnostic>> {
    let mir_body = mir.body(body);
    let valid = mir_body.source == BodySource::Method
        && method_of(tcx, mir_body.owner, mir_body.member).is_some_and(|sig| {
            sig.receiver.is_none()
                && sig.ret == Ty::Void
                && sig.params.is_empty()
                && sig.generics.is_empty()
        });
    if valid {
        return Ok(());
    }
    Err(Box::new(
        Diagnostic::error(format!(
            "`@Test` methods must be written as `static void {}()`",
            mir_body.name
        ))
        .with_span(mir_body.span)
        .with_note("a test takes no arguments and fails by throwing an exception"),
    ))
}

pub fn library_exports(
    mir: &Mir,
    tcx: &Tcx<'_>,
    prefix: &str,
    is_user: impl Fn(DefId) -> bool,
) -> Vec<crate::main_fn::Export> {
    let mut exports: Vec<crate::main_fn::Export> = Vec::new();
    for (index, body) in mir.bodies.iter().enumerate() {
        if body.source != BodySource::Method || !is_user(body.owner) {
            continue;
        }
        let Some(sig) = method_of(tcx, body.owner, body.member) else {
            continue;
        };
        let simple = |ty: &Ty| matches!(ty, Ty::Primitive(_) | Ty::Void);
        let exported = sig.receiver.is_none()
            && matches!(sig.vis, krelpin_ast::ast::Visibility::Public(_))
            && sig.generics.is_empty()
            && simple(&sig.ret)
            && sig.params.iter().all(simple);
        if !exported {
            continue;
        }
        let class = tcx.sig(body.owner).name.name;
        let base = format!("{prefix}_{class}_{}", body.name);
        let mut symbol = base.clone();
        let mut counter = 1;
        while exports.iter().any(|export| export.symbol == symbol) {
            counter += 1;
            symbol = format!("{base}_{counter}");
        }
        exports.push(crate::main_fn::Export {
            body: BodyId(index as u32),
            symbol,
        });
    }
    exports
}

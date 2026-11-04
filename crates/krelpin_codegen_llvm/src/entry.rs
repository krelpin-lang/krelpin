use krelpin_errors::Diagnostic;
use krelpin_mir::{BodyId, BodySource, Mir};
use krelpin_span::Symbol;
use krelpin_span::def_id::DefId;
use krelpin_typeck::{MethodSig, Tcx, Ty};

pub fn find_entry(
    mir: &Mir,
    tcx: &Tcx<'_>,
    is_user: impl Fn(DefId) -> bool,
) -> Result<BodyId, Diagnostic> {
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
        [] => Err(Diagnostic::error("this program has no `main` method")
            .with_help("add `static void main()` or `static void main(String[] args)` to a class")),
        [first, second, ..] => Err(Diagnostic::error(
            "this program has more than one `main` method",
        )
        .with_span(mir.body(*second).span)
        .with_secondary(mir.body(*first).span, "the first `main` is here")
        .with_help("keep a single `static void main()`")),
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

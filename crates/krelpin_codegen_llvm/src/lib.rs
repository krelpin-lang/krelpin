mod body;
mod builtins;
mod cx;
mod entry;
mod func;
mod glue;
mod instance;
mod layout;
mod link;
mod main_fn;
mod rtti;
mod thunk;

use krelpin_errors::DiagCtxt;
use krelpin_mir::{BodyId, Mir};
use krelpin_typeck::Tcx;

pub use crate::entry::find_entry;
pub use crate::link::{LinkOptions, OutputKind, link};

pub fn codegen(mir: &Mir, tcx: &Tcx<'_>, entry: BodyId, dcx: &DiagCtxt) -> Option<String> {
    let mut cx = cx::Cx::new(mir, tcx);
    cx.compile(entry);
    let failed = !cx.errors.is_empty();
    for error in cx.errors.drain(..) {
        dcx.emit(error);
    }
    if failed {
        return None;
    }
    Some(cx.finish())
}

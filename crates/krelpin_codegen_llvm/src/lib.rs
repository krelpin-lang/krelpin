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

#[cfg(test)]
mod tests;

use krelpin_errors::DiagCtxt;
use krelpin_mir::Mir;
use krelpin_typeck::Tcx;

pub use crate::entry::{check_test, find_entry, library_exports};
pub use crate::link::{LinkOptions, OutputKind, Target, link};
pub use crate::main_fn::{Entry, Export, TestCase};

pub fn codegen(
    mir: &Mir,
    tcx: &Tcx<'_>,
    entry: &Entry,
    target: &Target,
    dcx: &DiagCtxt,
) -> Option<String> {
    let mut cx = cx::Cx::new(mir, tcx);
    cx.compile(entry);
    let failed = !cx.errors.is_empty();
    for error in cx.errors.drain(..) {
        dcx.emit(error);
    }
    if failed {
        return None;
    }
    Some(format!(
        "target triple = \"{}\"\n{}",
        target.triple,
        cx.finish()
    ))
}

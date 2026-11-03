mod bitset;
mod check;
mod dataflow;
mod init;
mod liveness;
mod loans;
mod places;

#[cfg(test)]
mod tests;

use krelpin_errors::DiagCtxt;
use krelpin_mir::{BodyId, Mir};
use krelpin_typeck::Tcx;

use crate::check::Checker;
use crate::places::PlaceTys;

pub fn check(mir: &Mir, bodies: impl IntoIterator<Item = BodyId>, tcx: &Tcx<'_>, dcx: &DiagCtxt) {
    for id in bodies {
        let body = mir.body(id);
        let tys = PlaceTys { tcx, body };
        Checker::new(body, &tys, dcx).run();
    }
}

//! Which locals may hold what an owned parameter borrows.
//!
//! A parameter that isn't a reference can still hold references, like a
//! `Future` of a call that borrowed something. The loans behind them belong
//! to the caller, so the body sees none, and passing such a parameter on to
//! a method that keeps its argument forever would let the references
//! outlive what they point to. This analysis follows those parameters
//! through assignments and calls, so the checker can require that a
//! forwarded parameter is kept forever itself.

use krelpin_mir::{
    Body, Local, StatementKind, TerminatorKind, operand_locals_of, rvalue_locals_of,
};

use crate::bitset::BitSet;
use crate::places::{PlaceTys, base_local};

/// For each local, the owned parameters whose borrows it may hold.
pub(crate) struct Forwarded {
    sources: Vec<BitSet>,
}

impl Forwarded {
    pub(crate) fn compute(body: &Body, tys: &PlaceTys<'_, '_>) -> Forwarded {
        let count = body.locals.len();
        let mut sources = vec![BitSet::new(count); count];
        for arg in body.args() {
            let ty = &body.local(arg).ty;
            if !matches!(ty, krelpin_typeck::Ty::Ref { .. }) && tys.may_hold_refs(ty) {
                sources[arg.index()].insert(arg.index());
            }
        }
        // Flow-insensitive: a local holds whatever any assignment to it
        // could have brought in. That is conservative and cheap.
        let mut changed = true;
        while changed {
            changed = false;
            for block in &body.blocks {
                for statement in &block.statements {
                    let StatementKind::Assign(assign) = &statement.kind else {
                        continue;
                    };
                    let (place, rvalue) = assign.as_ref();
                    let Some(dest) = base_local(place) else {
                        continue;
                    };
                    let mut incoming = BitSet::new(count);
                    rvalue_locals_of(rvalue, &mut |local: Local| {
                        incoming.union(&sources[local.index()]);
                    });
                    changed |= sources[dest.index()].union(&incoming);
                }
                if let TerminatorKind::Call {
                    args,
                    dest: Some(dest),
                    ..
                } = &block.terminator.kind
                    && let Some(dest) = base_local(dest)
                    && tys.may_hold_refs(&body.local(dest).ty)
                {
                    let mut incoming = BitSet::new(count);
                    for arg in args {
                        operand_locals_of(arg, &mut |local: Local| {
                            incoming.union(&sources[local.index()]);
                        });
                    }
                    changed |= sources[dest.index()].union(&incoming);
                }
            }
        }
        Forwarded { sources }
    }

    /// The first owned parameter whose borrows `local` may hold.
    pub(crate) fn source(&self, local: Local) -> Option<Local> {
        self.sources[local.index()]
            .iter()
            .next()
            .map(|index| Local(index as u32))
    }
}

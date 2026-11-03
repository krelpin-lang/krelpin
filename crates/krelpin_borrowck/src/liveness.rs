use krelpin_mir::{
    BasicBlock, BasicBlockData, Body, Local, Place, StatementKind, TerminatorKind, place_locals_of,
    rvalue_locals_of, terminator_input_locals_of,
};

use crate::bitset::BitSet;
use crate::dataflow::Location;

pub(crate) struct Liveness {
    after: Vec<Vec<BitSet>>,
}

impl Liveness {
    pub(crate) fn compute(body: &Body) -> Liveness {
        let locals = body.locals.len();
        let blocks = body.blocks.len();
        let mut live_in = vec![BitSet::new(locals); blocks];
        let mut changed = true;
        while changed {
            changed = false;
            for index in (0..blocks).rev() {
                let data = body.block(BasicBlock(index as u32));
                let mut state = live_out(data, &live_in, locals);
                transfer_terminator(&data.terminator.kind, &mut state);
                for statement in data.statements.iter().rev() {
                    transfer_statement(&statement.kind, &mut state);
                }
                if state != live_in[index] {
                    live_in[index] = state;
                    changed = true;
                }
            }
        }

        let after = body
            .blocks
            .iter()
            .map(|data| {
                let mut points = vec![BitSet::new(locals); data.statements.len() + 1];
                let mut state = live_out(data, &live_in, locals);
                points[data.statements.len()] = state.clone();
                transfer_terminator(&data.terminator.kind, &mut state);
                for (index, statement) in data.statements.iter().enumerate().rev() {
                    points[index] = state.clone();
                    transfer_statement(&statement.kind, &mut state);
                }
                points
            })
            .collect();
        Liveness { after }
    }

    pub(crate) fn live_after(&self, location: Location) -> &BitSet {
        &self.after[location.block.index()][location.index]
    }
}

fn live_out(data: &BasicBlockData, live_in: &[BitSet], locals: usize) -> BitSet {
    let mut state = BitSet::new(locals);
    for successor in data.terminator.kind.successors() {
        state.union(&live_in[successor.index()]);
    }
    state
}

fn transfer_statement(kind: &StatementKind, state: &mut BitSet) {
    match kind {
        StatementKind::Assign(assign) => {
            let (place, rvalue) = assign.as_ref();
            define(place, state);
            let mut uses = Vec::new();
            rvalue_locals_of(rvalue, &mut |local| uses.push(local));
            if place.as_local().is_none() {
                place_locals(place, &mut uses);
            }
            for local in uses {
                state.insert(local.index());
            }
        }
        StatementKind::StorageLive(local) | StatementKind::StorageDead(local) => {
            state.remove(local.index());
        }
        StatementKind::FakeRead(place) => {
            let mut uses = Vec::new();
            place_locals(place, &mut uses);
            for local in uses {
                state.insert(local.index());
            }
        }
    }
}

fn transfer_terminator(kind: &TerminatorKind, state: &mut BitSet) {
    let mut uses = Vec::new();
    if let TerminatorKind::Call {
        dest: Some(dest), ..
    } = kind
    {
        define(dest, state);
        if dest.as_local().is_none() {
            place_locals(dest, &mut uses);
        }
    }
    terminator_input_locals_of(kind, &mut |local| uses.push(local));
    for local in uses {
        state.insert(local.index());
    }
}

fn define(place: &Place, state: &mut BitSet) {
    if let Some(local) = place.as_local() {
        state.remove(local.index());
    }
}

fn place_locals(place: &Place, out: &mut Vec<Local>) {
    place_locals_of(place, &mut |local| out.push(local));
}

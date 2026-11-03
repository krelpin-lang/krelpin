use std::collections::VecDeque;

use krelpin_mir::{BasicBlock, Body, StatementKind, TerminatorKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Location {
    pub(crate) block: BasicBlock,
    pub(crate) index: usize,
}

pub(crate) trait Forward {
    type State: Clone;

    fn bottom(&self) -> Self::State;
    fn entry(&self) -> Self::State;
    fn join(&self, into: &mut Self::State, from: &Self::State) -> bool;
    fn statement(&self, state: &mut Self::State, location: Location, kind: &StatementKind);
    fn terminator(&self, state: &mut Self::State, location: Location, kind: &TerminatorKind);

    fn on_return_edge(&self, _state: &mut Self::State, _kind: &TerminatorKind) {}
}

pub(crate) fn solve<A: Forward>(analysis: &A, body: &Body) -> Vec<A::State> {
    let mut entries: Vec<A::State> = vec![analysis.bottom(); body.blocks.len()];
    if body.blocks.is_empty() {
        return entries;
    }
    entries[0] = analysis.entry();
    let mut queue: VecDeque<BasicBlock> = (0..body.blocks.len())
        .map(|index| BasicBlock(index as u32))
        .collect();
    let mut queued = vec![true; body.blocks.len()];
    while let Some(block) = queue.pop_front() {
        queued[block.index()] = false;
        let mut state = entries[block.index()].clone();
        let data = body.block(block);
        for (index, statement) in data.statements.iter().enumerate() {
            analysis.statement(&mut state, Location { block, index }, &statement.kind);
        }
        let location = Location {
            block,
            index: data.statements.len(),
        };
        analysis.terminator(&mut state, location, &data.terminator.kind);
        for (successor, edge_state) in edge_states(analysis, &state, &data.terminator.kind) {
            if analysis.join(&mut entries[successor.index()], &edge_state)
                && !queued[successor.index()]
            {
                queued[successor.index()] = true;
                queue.push_back(successor);
            }
        }
    }
    entries
}

pub(crate) fn edge_states<A: Forward>(
    analysis: &A,
    state: &A::State,
    kind: &TerminatorKind,
) -> Vec<(BasicBlock, A::State)> {
    match kind {
        TerminatorKind::Call { target, unwind, .. } => {
            let mut returned = state.clone();
            analysis.on_return_edge(&mut returned, kind);
            let mut out = vec![(*target, returned)];
            if let krelpin_mir::Unwind::Handler(handler) = unwind {
                out.push((*handler, state.clone()));
            }
            out
        }
        _ => kind
            .successors()
            .into_iter()
            .map(|successor| (successor, state.clone()))
            .collect(),
    }
}

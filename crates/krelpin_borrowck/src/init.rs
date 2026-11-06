use std::collections::HashMap;

use krelpin_mir::{Body, Local, Place, StatementKind, TerminatorKind};
use krelpin_span::Span;

use crate::bitset::BitSet;
use crate::dataflow::{Forward, Location};
use crate::places::{self, Access, PlacePath};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MoveKind {
    Uninit,
    Moved,
}

#[derive(Clone, Debug)]
pub(crate) struct MoveOut {
    pub(crate) path: PlacePath,
    pub(crate) kind: MoveKind,
    pub(crate) span: Span,
}

pub(crate) struct MoveData {
    pub(crate) moves: Vec<MoveOut>,
    at: HashMap<(Location, PlacePath), usize>,
    arg_count: usize,
    local_count: usize,
}

impl MoveData {
    pub(crate) fn collect(body: &Body) -> MoveData {
        let mut moves: Vec<MoveOut> = (0..body.locals.len())
            .map(|index| MoveOut {
                path: PlacePath {
                    local: Local(index as u32),
                    elems: Vec::new(),
                    indirect: false,
                },
                kind: MoveKind::Uninit,
                span: body.locals[index].span,
            })
            .collect();
        let mut at = HashMap::new();
        for block_index in 0..body.blocks.len() {
            let block = krelpin_mir::BasicBlock(block_index as u32);
            let data = body.block(block);
            let sites = data
                .statements
                .iter()
                .map(|statement| (places::statement_accesses(&statement.kind), statement.span))
                .chain(std::iter::once((
                    places::terminator_accesses(&data.terminator.kind),
                    data.terminator.span,
                )));
            for (index, (accesses, span)) in sites.enumerate() {
                let location = Location { block, index };
                for (place, access) in accesses {
                    if access != Access::Move {
                        continue;
                    }
                    if let Some(path) = PlacePath::of(&place).filter(|path| !path.indirect) {
                        at.entry((location, path.clone())).or_insert_with(|| {
                            moves.push(MoveOut {
                                path,
                                kind: MoveKind::Moved,
                                span,
                            });
                            moves.len() - 1
                        });
                    }
                }
            }
        }
        MoveData {
            moves,
            at,
            // The `Context` of an `async` body comes in like an argument.
            arg_count: body.arg_count + usize::from(body.asyncness.is_some()),
            local_count: body.locals.len(),
        }
    }

    pub(crate) fn conflicting(
        &self,
        state: &BitSet,
        path: &PlacePath,
        access: Access,
    ) -> Option<&MoveOut> {
        state.iter().map(|index| &self.moves[index]).find(|moved| {
            if moved.path.local != path.local {
                return false;
            }
            match access {
                Access::Write if !path.indirect => {
                    moved.path.is_prefix_of(path) && moved.path.elems.len() < path.elems.len()
                }
                _ => moved.path.related(path),
            }
        })
    }

    fn gen_moves(&self, state: &mut BitSet, location: Location, accesses: &[(Place, Access)]) {
        for (place, access) in accesses {
            if *access != Access::Move {
                continue;
            }
            if let Some(path) = PlacePath::of(place)
                && let Some(&index) = self.at.get(&(location, path))
            {
                state.insert(index);
            }
        }
    }

    fn init(&self, state: &mut BitSet, place: &Place) {
        let Some(path) = PlacePath::of(place).filter(|path| !path.indirect) else {
            return;
        };
        for (index, moved) in self.moves.iter().enumerate() {
            if path.is_prefix_of(&moved.path) {
                state.remove(index);
            }
        }
    }

    fn reset_local(&self, state: &mut BitSet, local: Local) {
        for (index, moved) in self.moves.iter().enumerate() {
            if moved.path.local == local {
                state.remove(index);
            }
        }
        state.insert(local.index());
    }
}

impl Forward for MoveData {
    type State = BitSet;

    fn bottom(&self) -> BitSet {
        BitSet::new(self.moves.len())
    }

    fn entry(&self) -> BitSet {
        let mut state = self.bottom();
        for local in 0..self.local_count {
            if local == 0 || local > self.arg_count {
                state.insert(local);
            }
        }
        state
    }

    fn join(&self, into: &mut BitSet, from: &BitSet) -> bool {
        into.union(from)
    }

    fn statement(&self, state: &mut BitSet, location: Location, kind: &StatementKind) {
        match kind {
            StatementKind::Assign(assign) => {
                let mut accesses = Vec::new();
                places::rvalue_accesses(&assign.1, &mut accesses);
                self.gen_moves(state, location, &accesses);
                self.init(state, &assign.0);
            }
            StatementKind::StorageLive(local) | StatementKind::StorageDead(local) => {
                self.reset_local(state, *local);
            }
            StatementKind::FakeRead(_) => {}
        }
    }

    fn terminator(&self, state: &mut BitSet, location: Location, kind: &TerminatorKind) {
        let accesses: Vec<(Place, Access)> = places::terminator_accesses(kind)
            .into_iter()
            .filter(|(_, access)| *access != Access::Write)
            .collect();
        self.gen_moves(state, location, &accesses);
    }

    fn on_return_edge(&self, state: &mut BitSet, kind: &TerminatorKind) {
        if let TerminatorKind::Call {
            dest: Some(dest), ..
        } = kind
        {
            self.init(state, dest);
        }
    }
}

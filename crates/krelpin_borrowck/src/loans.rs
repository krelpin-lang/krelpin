use std::collections::{HashMap, HashSet};

use krelpin_ast::ast::ReceiverKind;
use krelpin_mir::{
    AggregateKind, BasicBlock, Body, Operand, Place, Rvalue, StatementKind, TerminatorKind,
};
use krelpin_span::Span;
use krelpin_typeck::{Callee, Ty};

use crate::bitset::BitSet;
use crate::dataflow::{Forward, Location};
use crate::places::{PlaceTys, base_local, has_deref, operand_place};

#[derive(Debug)]
pub(crate) struct Loan {
    pub(crate) place: Place,
    pub(crate) mutable: bool,
    pub(crate) span: Span,
    pub(crate) captured: bool,
}

pub(crate) struct LoanData<'a, 'tcx> {
    pub(crate) loans: Vec<Loan>,
    at: HashMap<Location, usize>,
    tys: &'a PlaceTys<'a, 'tcx>,
    holds_refs: Vec<bool>,
}

pub(crate) type Holdings = Vec<BitSet>;

impl<'a, 'tcx> LoanData<'a, 'tcx> {
    pub(crate) fn collect(body: &Body, tys: &'a PlaceTys<'a, 'tcx>) -> LoanData<'a, 'tcx> {
        let mut captured = HashSet::new();
        for block in &body.blocks {
            for statement in &block.statements {
                if let StatementKind::Assign(assign) = &statement.kind
                    && let Rvalue::Aggregate(kind, operands) = &assign.1
                    && let AggregateKind::Closure(_) = kind.as_ref()
                {
                    captured.extend(
                        operands
                            .iter()
                            .filter_map(operand_place)
                            .filter_map(Place::as_local),
                    );
                }
            }
        }

        let mut loans = Vec::new();
        let mut at = HashMap::new();
        for (block_index, block) in body.blocks.iter().enumerate() {
            for (index, statement) in block.statements.iter().enumerate() {
                let StatementKind::Assign(assign) = &statement.kind else {
                    continue;
                };
                let (dest, Rvalue::Ref { mutable, place }) = assign.as_ref() else {
                    continue;
                };
                let location = Location {
                    block: BasicBlock(block_index as u32),
                    index,
                };
                at.insert(location, loans.len());
                loans.push(Loan {
                    place: place.clone(),
                    mutable: *mutable,
                    span: statement.span,
                    captured: dest
                        .as_local()
                        .is_some_and(|local| captured.contains(&local)),
                });
            }
        }
        let holds_refs = body
            .locals
            .iter()
            .map(|decl| tys.may_hold_refs(&decl.ty))
            .collect();
        LoanData {
            loans,
            at,
            tys,
            holds_refs,
        }
    }

    fn operand_loans(&self, state: &Holdings, operand: &Operand) -> BitSet {
        let mut held = BitSet::new(self.loans.len());
        if let Some(local) = operand_place(operand).and_then(base_local) {
            held.union(&state[local.index()]);
        }
        held
    }

    fn rvalue_loans(&self, state: &Holdings, location: Location, rvalue: &Rvalue) -> BitSet {
        let mut held = BitSet::new(self.loans.len());
        match rvalue {
            Rvalue::Ref { place, .. } => {
                if let Some(&loan) = self.at.get(&location) {
                    held.insert(loan);
                }
                if has_deref(place)
                    && let Some(local) = base_local(place)
                {
                    held.union(&state[local.index()]);
                }
            }
            Rvalue::Use(operand) | Rvalue::Cast(operand, _) => {
                held.union(&self.operand_loans(state, operand));
            }
            Rvalue::Aggregate(_, operands) => {
                for operand in operands {
                    held.union(&self.operand_loans(state, operand));
                }
            }
            _ => {}
        }
        held
    }

    fn store(&self, state: &mut Holdings, place: &Place, held: BitSet) {
        match place.as_local() {
            Some(local) => {
                state[local.index()] = if self.holds_refs[local.index()] {
                    held
                } else {
                    BitSet::new(self.loans.len())
                };
            }
            None => {
                if held.is_empty() || !self.tys.may_hold_refs(&self.tys.place_ty(place)) {
                    return;
                }
                if let Some(local) = base_local(place) {
                    state[local.index()].union(&held);
                }
            }
        }
    }
}

impl Forward for LoanData<'_, '_> {
    type State = Holdings;

    fn bottom(&self) -> Holdings {
        vec![BitSet::new(self.loans.len()); self.holds_refs.len()]
    }

    fn entry(&self) -> Holdings {
        self.bottom()
    }

    fn join(&self, into: &mut Holdings, from: &Holdings) -> bool {
        let mut changed = false;
        for (into, from) in into.iter_mut().zip(from) {
            changed |= into.union(from);
        }
        changed
    }

    fn statement(&self, state: &mut Holdings, location: Location, kind: &StatementKind) {
        match kind {
            StatementKind::Assign(assign) => {
                let (place, rvalue) = assign.as_ref();
                let held = self.rvalue_loans(state, location, rvalue);
                self.store(state, place, held);
            }
            StatementKind::StorageLive(local) | StatementKind::StorageDead(local) => {
                state[local.index()].clear();
            }
            StatementKind::FakeRead(_) => {}
        }
    }

    fn terminator(&self, state: &mut Holdings, _location: Location, kind: &TerminatorKind) {
        let TerminatorKind::Call { callee, args, .. } = kind else {
            return;
        };
        let passed: Vec<Passed> = (0..args.len())
            .map(|index| self.passed(state, callee, args, index))
            .collect();
        for (index, arg) in passed.iter().enumerate() {
            if !matches!(arg.declared, DeclaredArg::Ref { mutable: true }) {
                continue;
            }
            let mut stored = BitSet::new(self.loans.len());
            for (other, value) in passed.iter().enumerate() {
                if other != index {
                    stored.union(value.stored());
                }
            }
            if stored.is_empty() {
                continue;
            }
            for loan in arg.held.iter() {
                let loan = &self.loans[loan];
                if !loan.mutable || !self.tys.may_hold_refs(&self.tys.place_ty(&loan.place)) {
                    continue;
                }
                if let Some(local) = base_local(&loan.place) {
                    state[local.index()].union(&stored);
                }
            }
        }
    }

    fn on_return_edge(&self, state: &mut Holdings, kind: &TerminatorKind) {
        let TerminatorKind::Call {
            callee,
            args,
            dest: Some(dest),
            ..
        } = kind
        else {
            return;
        };
        let mut held = BitSet::new(self.loans.len());
        if self.tys.may_hold_refs(&self.tys.place_ty(dest)) {
            let returns_ref = contains_ref(&callee.method.ret);
            for index in 0..args.len() {
                let passed = self.passed(state, callee, args, index);
                if returns_ref {
                    held.union(&passed.held);
                } else {
                    held.union(&passed.contents);
                }
            }
        }
        self.store(state, dest, held);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeclaredArg {
    Value,
    Ref { mutable: bool },
    DeclaredRef,
}

struct Passed {
    declared: DeclaredArg,
    held: BitSet,
    contents: BitSet,
}

impl Passed {
    fn stored(&self) -> &BitSet {
        match self.declared {
            DeclaredArg::Value | DeclaredArg::DeclaredRef => &self.held,
            DeclaredArg::Ref { .. } => &self.contents,
        }
    }
}

impl LoanData<'_, '_> {
    fn passed(&self, state: &Holdings, callee: &Callee, args: &[Operand], index: usize) -> Passed {
        let has_receiver = args.len() > callee.method.params.len();
        let declared = match (has_receiver, index) {
            (true, 0) => match callee.method.receiver.unwrap_or(ReceiverKind::RefMut) {
                ReceiverKind::Owned => DeclaredArg::Value,
                ReceiverKind::Ref => DeclaredArg::Ref { mutable: false },
                ReceiverKind::RefMut => DeclaredArg::Ref { mutable: true },
            },
            _ => {
                let param = index - usize::from(has_receiver);
                match callee.method.params.get(param) {
                    Some(Ty::Ref { mutable, .. }) => DeclaredArg::Ref { mutable: *mutable },
                    Some(ty) if contains_ref(ty) => DeclaredArg::DeclaredRef,
                    _ => DeclaredArg::Value,
                }
            }
        };
        let held = self.operand_loans(state, &args[index]);
        let contents = match declared {
            DeclaredArg::Ref { .. } => {
                let mut contents = BitSet::new(self.loans.len());
                for loan in held.iter() {
                    if let Some(local) = base_local(&self.loans[loan].place) {
                        contents.union(&state[local.index()]);
                    }
                }
                contents
            }
            DeclaredArg::Value | DeclaredArg::DeclaredRef => held.clone(),
        };
        Passed {
            declared,
            held,
            contents,
        }
    }
}

fn contains_ref(ty: &Ty) -> bool {
    match ty {
        Ty::Ref { .. } => true,
        Ty::Nullable(inner) | Ty::Array(inner) => contains_ref(inner),
        Ty::Adt { args, .. } | Ty::External { args, .. } => args.iter().any(contains_ref),
        Ty::Primitive(_) | Ty::Void | Ty::Null | Ty::Param(_) | Ty::Unknown => false,
    }
}

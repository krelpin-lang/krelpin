use std::collections::{HashMap, HashSet};

use krelpin_ast::ast::ReceiverKind;
use krelpin_mir::{
    AggregateKind, BasicBlock, Body, Local, Operand, Place, PlaceElem, Rvalue, StatementKind,
    TerminatorKind,
};
use krelpin_span::Span;
use krelpin_typeck::{BorrowSource, Callee, Storage, Ty};

use crate::bitset::BitSet;
use crate::dataflow::{Forward, Location};
use crate::places::{PlaceTys, base_local, operand_place};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoanOrigin {
    Local,
    Arg(Local),
    ArgContents(Local),
}

#[derive(Debug)]
pub(crate) struct Loan {
    pub(crate) place: Place,
    pub(crate) mutable: bool,
    pub(crate) span: Span,
    pub(crate) captured: bool,
    pub(crate) origin: LoanOrigin,
}

pub(crate) struct LoanData<'a, 'tcx> {
    pub(crate) loans: Vec<Loan>,
    at: HashMap<Location, usize>,
    arg_loans: Vec<(Local, usize)>,
    contents_loans: HashMap<Local, usize>,
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
        let mut arg_loans = Vec::new();
        let mut contents_loans = HashMap::new();
        for local in body.args() {
            let decl = body.local(local);
            let Ty::Ref { mutable, .. } = decl.ty else {
                continue;
            };
            let place = Place::local(local).project(PlaceElem::Deref);
            arg_loans.push((local, loans.len()));
            loans.push(Loan {
                place: place.clone(),
                mutable,
                span: decl.span,
                captured: false,
                origin: LoanOrigin::Arg(local),
            });
            contents_loans.insert(local, loans.len());
            loans.push(Loan {
                place,
                mutable,
                span: decl.span,
                captured: false,
                origin: LoanOrigin::ArgContents(local),
            });
        }
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
                    origin: LoanOrigin::Local,
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
            arg_loans,
            contents_loans,
            tys,
            holds_refs,
        }
    }

    fn operand_loans(&self, state: &Holdings, operand: &Operand) -> BitSet {
        match operand_place(operand) {
            Some(place) => self.place_loans(state, place),
            None => BitSet::new(self.loans.len()),
        }
    }

    fn place_loans(&self, state: &Holdings, place: &Place) -> BitSet {
        self.place_loans_within(state, place, &mut BitSet::new(self.loans.len()))
    }

    fn place_loans_within(&self, state: &Holdings, place: &Place, active: &mut BitSet) -> BitSet {
        let Some(local) = base_local(place) else {
            return BitSet::new(self.loans.len());
        };
        match referent(place) {
            Some(reference) => {
                let references = self.place_loans_within(state, &reference, active);
                self.contents_within(state, &references, active)
            }
            None => state[local.index()].clone(),
        }
    }

    fn contents(&self, state: &Holdings, held: &BitSet) -> BitSet {
        self.contents_within(state, held, &mut BitSet::new(self.loans.len()))
    }

    fn contents_within(&self, state: &Holdings, held: &BitSet, active: &mut BitSet) -> BitSet {
        let mut contents = BitSet::new(self.loans.len());
        for index in held.iter() {
            let loan = &self.loans[index];
            match loan.origin {
                LoanOrigin::Arg(local) | LoanOrigin::ArgContents(local) => {
                    if let Some(&index) = self.contents_loans.get(&local) {
                        contents.insert(index);
                    }
                }
                LoanOrigin::Local => {
                    if active.contains(index) {
                        continue;
                    }
                    active.insert(index);
                    contents.union(&self.place_loans_within(state, &loan.place, active));
                    active.remove(index);
                }
            }
        }
        contents
    }

    fn rvalue_loans(&self, state: &Holdings, location: Location, rvalue: &Rvalue) -> BitSet {
        let mut held = BitSet::new(self.loans.len());
        match rvalue {
            Rvalue::Ref { place, .. } => {
                if let Some(&loan) = self.at.get(&location) {
                    held.insert(loan);
                }
                if let Some(reference) = referent(place) {
                    held.union(&self.place_loans(state, &reference));
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
        let mut state = self.bottom();
        for &(local, loan) in &self.arg_loans {
            state[local.index()].insert(loan);
        }
        state
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
        for (index, arg) in args.iter().enumerate() {
            let is_closure = operand_place(arg)
                .map(|place| self.tys.place_ty(place))
                .is_some_and(|ty| self.tys.tcx.fn_receiver(&ty).is_some());
            if !is_closure {
                continue;
            }
            let mut handed = BitSet::new(self.loans.len());
            for (other, value) in passed.iter().enumerate() {
                if other != index {
                    handed.union(&value.held);
                    handed.union(&value.contents);
                }
            }
            if handed.is_empty() {
                continue;
            }
            for loan in passed[index].held.iter() {
                let loan = &self.loans[loan];
                let holds = self.tys.may_hold_refs(&self.tys.place_ty(&loan.place));
                if loan.mutable
                    && loan.origin == LoanOrigin::Local
                    && holds
                    && let Some(local) = base_local(&loan.place)
                {
                    state[local.index()].union(&handed);
                }
            }
        }
        for (index, arg) in passed.iter().enumerate() {
            if !matches!(arg.declared, DeclaredArg::Ref { mutable: true }) {
                continue;
            }
            let mut stored = BitSet::new(self.loans.len());
            for (other, value) in passed.iter().enumerate() {
                if other != index {
                    stored.union(&value.stored());
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
            held = self.returned_loans(state, callee, args);
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
    storage: Option<Storage>,
    held: BitSet,
    contents: BitSet,
}

impl Passed {
    fn stored(&self) -> BitSet {
        let mut stored = BitSet::new(self.held.len());
        let Some(storage) = self.storage else {
            return stored;
        };
        if storage.whole {
            stored.union(&self.held);
        }
        if storage.contents {
            stored.union(&self.contents);
        }
        stored
    }
}

impl LoanData<'_, '_> {
    fn returned_loans(&self, state: &Holdings, callee: &Callee, args: &[Operand]) -> BitSet {
        let mut held = BitSet::new(self.loans.len());
        let borrows = &callee.method.borrows;
        let has_receiver = args.len() > callee.method.params.len();
        let Some(sources) = borrows.explicit() else {
            let borrowing = borrows.elided || contains_ref(&callee.method.ret);
            for index in 0..args.len() {
                if borrowing {
                    held.union(&self.passed(state, callee, args, index).held);
                } else {
                    held.union(&self.element_loans(state, callee, args, index));
                }
            }
            return held;
        };
        for source in sources {
            let (index, contents) = match source {
                BorrowSource::This => (0, false),
                BorrowSource::Param(param) => (param + usize::from(has_receiver), false),
                BorrowSource::Class(_) => (0, true),
            };
            if index >= args.len() {
                continue;
            }
            let passed = self.passed(state, callee, args, index);
            held.union(if contents {
                &passed.contents
            } else {
                &passed.held
            });
        }
        if contains_param(&callee.method.ret) {
            for index in 0..args.len() {
                held.union(&self.element_loans(state, callee, args, index));
            }
        }
        held
    }

    fn element_loans(
        &self,
        state: &Holdings,
        callee: &Callee,
        args: &[Operand],
        index: usize,
    ) -> BitSet {
        let contents = self.passed(state, callee, args, index).contents;
        let viewed = operand_place(&args[index])
            .map(|place| self.tys.place_ty(place))
            .is_some_and(|ty| self.tys.is_view(ty.strip_refs()));
        if viewed {
            self.contents(state, &contents)
        } else {
            contents
        }
    }

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
        let storage = match (has_receiver, index) {
            (true, 0) => None,
            _ => Some(
                callee
                    .method
                    .storage
                    .get(index - usize::from(has_receiver))
                    .copied()
                    .unwrap_or(Storage {
                        whole: true,
                        contents: true,
                    }),
            ),
        };
        let held = self.operand_loans(state, &args[index]);
        let contents = match declared {
            DeclaredArg::Ref { .. } => self.contents(state, &held),
            DeclaredArg::Value | DeclaredArg::DeclaredRef => held.clone(),
        };
        Passed {
            declared,
            storage,
            held,
            contents,
        }
    }
}

fn referent(place: &Place) -> Option<Place> {
    let at = place
        .projection
        .iter()
        .rposition(|elem| matches!(elem, PlaceElem::Deref))?;
    Some(Place {
        base: place.base,
        projection: place.projection[..at].to_vec(),
    })
}

fn contains_param(ty: &Ty) -> bool {
    match ty {
        Ty::Param(_) => true,
        Ty::Ref { ty, .. } | Ty::Nullable(ty) | Ty::Array(ty) => contains_param(ty),
        Ty::Adt { args, .. } => args.iter().any(contains_param),
        Ty::Primitive(_) | Ty::Void | Ty::Null | Ty::Unknown => false,
    }
}

fn contains_ref(ty: &Ty) -> bool {
    match ty {
        Ty::Ref { .. } => true,
        Ty::Nullable(inner) | Ty::Array(inner) => contains_ref(inner),
        Ty::Adt { args, .. } => args.iter().any(contains_ref),
        Ty::Primitive(_) | Ty::Void | Ty::Null | Ty::Param(_) | Ty::Unknown => false,
    }
}

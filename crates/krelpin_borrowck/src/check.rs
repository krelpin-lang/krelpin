use std::collections::{HashSet, VecDeque};

use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_mir::{
    BasicBlock, Body, Local, Place, PlaceBase, PlaceElem, StatementKind, TerminatorKind, Unwind,
    place_locals_of, rvalue_locals_of, terminator_input_locals_of,
};
use krelpin_span::Span;
use krelpin_typeck::Ty;

use crate::bitset::BitSet;
use crate::dataflow::{Forward, Location, solve};
use crate::init::{MoveData, MoveKind, MoveOut};
use crate::liveness::Liveness;
use crate::loans::{Holdings, LoanData};
use crate::places::{
    self, Access, PlacePath, PlaceTys, base_local, has_deref, operand_place, overlaps,
};

pub(crate) struct Checker<'a, 'tcx> {
    body: &'a Body,
    tys: &'a PlaceTys<'a, 'tcx>,
    moves: MoveData,
    loans: LoanData<'a, 'tcx>,
    liveness: Liveness,
    dcx: &'a DiagCtxt,
    reported: HashSet<(Span, String)>,
    reported_places: HashSet<(Location, String)>,
}

impl<'a, 'tcx> Checker<'a, 'tcx> {
    pub(crate) fn new(body: &'a Body, tys: &'a PlaceTys<'a, 'tcx>, dcx: &'a DiagCtxt) -> Self {
        Checker {
            body,
            tys,
            moves: MoveData::collect(body),
            loans: LoanData::collect(body, tys),
            liveness: Liveness::compute(body),
            dcx,
            reported: HashSet::new(),
            reported_places: HashSet::new(),
        }
    }

    pub(crate) fn run(mut self) {
        let init_entries = solve(&self.moves, self.body);
        let loan_entries = solve(&self.loans, self.body);
        for (index, data) in self.body.blocks.iter().enumerate() {
            let block = BasicBlock(index as u32);
            let mut init = init_entries[index].clone();
            let mut held = loan_entries[index].clone();
            for (index, statement) in data.statements.iter().enumerate() {
                let location = Location { block, index };
                self.check_statement(location, &statement.kind, statement.span, &init, &held);
                self.moves.statement(&mut init, location, &statement.kind);
                self.loans.statement(&mut held, location, &statement.kind);
            }
            let location = Location {
                block,
                index: data.statements.len(),
            };
            self.check_terminator(
                location,
                &data.terminator.kind,
                data.terminator.span,
                &init,
                &held,
            );
        }
    }

    fn check_statement(
        &mut self,
        location: Location,
        kind: &StatementKind,
        span: Span,
        init: &BitSet,
        held: &Holdings,
    ) {
        match kind {
            StatementKind::Assign(_) | StatementKind::FakeRead(_) => {
                for (place, access) in places::statement_accesses(kind) {
                    self.check_access(location, span, &place, access, init, held);
                }
            }
            StatementKind::StorageDead(local) => {
                let live = self.live_loans(location, held);
                let dangling = live.iter().find(|&loan| {
                    let place = &self.loans.loans[loan].place;
                    base_local(place) == Some(*local) && !has_deref(place)
                });
                if let Some(loan) = dangling {
                    self.report_dangling(loan, span);
                }
            }
            StatementKind::StorageLive(_) => {}
        }
    }

    fn check_terminator(
        &mut self,
        location: Location,
        kind: &TerminatorKind,
        span: Span,
        init: &BitSet,
        held: &Holdings,
    ) {
        for (place, access) in places::terminator_accesses(kind) {
            self.check_access(location, span, &place, access, init, held);
        }
        match kind {
            TerminatorKind::Return => {
                let mut escaping = held[Local::RETURN.index()].clone();
                for arg in self.body.args() {
                    if matches!(self.body.local(arg).ty, Ty::Ref { mutable: true, .. }) {
                        escaping.union(&held[arg.index()]);
                    }
                }
                if let Some(loan) = escaping.iter().find(|&loan| self.is_local_data(loan)) {
                    self.report_escape(loan, span, "returned");
                }
            }
            TerminatorKind::Throw {
                value,
                unwind: Unwind::Resume,
            } => {
                let escaping = operand_place(value)
                    .and_then(base_local)
                    .map(|local| held[local.index()].clone());
                let loan = escaping
                    .and_then(|escaping| escaping.iter().find(|&loan| self.is_local_data(loan)));
                if let Some(loan) = loan {
                    self.report_escape(loan, span, "thrown");
                }
            }
            _ => {}
        }
    }

    fn is_local_data(&self, loan: usize) -> bool {
        let place = &self.loans.loans[loan].place;
        matches!(place.base, PlaceBase::Local(_)) && !has_deref(place)
    }

    fn live_loans(&self, location: Location, held: &Holdings) -> BitSet {
        let defined = self.defined_at(location);
        let mut live = BitSet::new(self.loans.loans.len());
        for local in self.liveness.live_after(location).iter() {
            if Some(local) != defined {
                live.union(&held[local]);
            }
        }
        live
    }

    fn defined_at(&self, location: Location) -> Option<usize> {
        let data = self.body.block(location.block);
        let place = match data.statements.get(location.index) {
            Some(statement) => match &statement.kind {
                StatementKind::Assign(assign) => Some(&assign.0),
                _ => None,
            },
            None => match &data.terminator.kind {
                TerminatorKind::Call { dest, .. } => dest.as_ref(),
                _ => None,
            },
        };
        place.and_then(Place::as_local).map(Local::index)
    }

    fn check_access(
        &mut self,
        location: Location,
        span: Span,
        place: &Place,
        access: Access,
        init: &BitSet,
        held: &Holdings,
    ) {
        let path = PlacePath::of(place);
        if let Some(path) = &path
            && let Some(moved) = self.moves.conflicting(init, path, access).cloned()
        {
            self.report_moved(location, span, place, access, &moved);
            return;
        }
        if access == Access::Move && path.as_ref().is_none_or(|path| path.indirect) {
            self.report_move_out(span, place);
            return;
        }
        if access.mutates()
            && let Some(immutable) = self.tys.immutable_prefix(place)
        {
            self.report_immutable(span, place, access, &immutable);
            return;
        }
        let live = self.live_loans(location, held);
        let conflict = live.iter().find(|&loan| {
            let loan = &self.loans.loans[loan];
            overlaps(&loan.place, place)
                && (loan.mutable
                    || !matches!(access, Access::Read | Access::Borrow { mutable: false }))
        });
        if let Some(loan) = conflict {
            self.report_conflict(location, span, place, access, loan, held);
        }
    }

    fn emit(&mut self, span: Span, diagnostic: Diagnostic) {
        if self.reported.insert((span, diagnostic.message.clone())) {
            self.dcx.emit(diagnostic);
        }
    }

    fn describe_path(&self, path: &PlacePath) -> String {
        let mut out = self
            .body
            .local(path.local)
            .name
            .map_or_else(|| "value".to_owned(), |name| name.to_string());
        for elem in &path.elems {
            match elem {
                places::PathElem::Field(_, name) => {
                    out.push('.');
                    out.push_str(name.as_str());
                }
                places::PathElem::Capture(index) => {
                    out = self.body.captures[*index].name.to_string();
                }
            }
        }
        out
    }

    fn report_moved(
        &mut self,
        location: Location,
        span: Span,
        place: &Place,
        access: Access,
        moved: &MoveOut,
    ) {
        let name = self.tys.describe(place);
        if !self.reported_places.insert((location, name.clone())) {
            return;
        }
        if moved.kind == MoveKind::Uninit {
            let diagnostic = Diagnostic::error(format!("`{name}` is used before it has a value"))
                .with_span(span)
                .with_label(format!(
                    "`{name}` is used here, but it may not have been assigned"
                ))
                .with_secondary(moved.span, "declared here");
            self.emit(span, diagnostic);
            return;
        }
        let moved_name = self.describe_path(&moved.path);
        let partial =
            moved.path.elems.len() > PlacePath::of(place).map_or(0, |path| path.elems.len());
        let verb = match access {
            Access::Borrow { .. } => "borrow",
            Access::Write => "assignment",
            Access::Read | Access::Move => "use",
        };
        let message = if partial {
            format!("{verb} of partially moved value: `{name}`")
        } else {
            format!("{verb} of moved value: `{name}`")
        };
        let moved_label = if moved.span == span {
            "value moved here, in an earlier iteration of the loop".to_owned()
        } else if partial {
            format!("`{moved_name}` was moved here")
        } else {
            "value moved here".to_owned()
        };
        let mut diagnostic = Diagnostic::error(message)
            .with_span(span)
            .with_label(format!("value {} here after move", past(access)));
        if moved.span != span {
            diagnostic = diagnostic.with_secondary(moved.span, moved_label);
        } else {
            diagnostic = diagnostic.with_label(moved_label);
        }
        let ty = self.tys.place_ty(&path_place(&moved.path));
        if !ty.is_unknown() {
            diagnostic = diagnostic.with_note(format!(
                "`{moved_name}` has type `{ty}`, which is moved instead of copied"
            ));
        }
        self.emit(span, diagnostic);
    }

    fn report_move_out(&mut self, span: Span, place: &Place) {
        let name = self.tys.describe(place);
        let ty = self.tys.place_ty(place);
        let first = place
            .projection
            .iter()
            .find(|elem| matches!(elem, PlaceElem::Deref | PlaceElem::Index(_)));
        let diagnostic = if let PlaceBase::Static { .. } = place.base {
            Diagnostic::error(format!("cannot move out of the static field `{name}`"))
                .with_span(span)
                .with_label("value moved here")
                .with_help(format!("borrow it instead: `&{name}`"))
        } else if self.tys.is_capture(place) {
            Diagnostic::error(format!("cannot move `{name}` out of a lambda"))
                .with_span(span)
                .with_label("value moved here")
                .with_note(
                    "the lambda can be called more than once, \
                     so it can only copy or borrow what it captured",
                )
        } else if let Some(PlaceElem::Index(_)) = first {
            let index = place
                .projection
                .iter()
                .position(|elem| matches!(elem, PlaceElem::Index(_)))
                .unwrap_or(place.projection.len());
            let array = self.tys.describe(&Place {
                base: place.base,
                projection: place.projection[..index].to_vec(),
            });
            Diagnostic::error(format!("cannot move out of an element of `{array}`"))
                .with_span(span)
                .with_label("value moved here")
                .with_note(format!("`{ty}` values are moved instead of copied"))
                .with_help("borrow the element instead")
        } else {
            Diagnostic::error(format!("cannot move out of `{name}`, which is borrowed"))
                .with_span(span)
                .with_label("value moved here")
                .with_note(format!("`{ty}` values are moved instead of copied"))
                .with_help(format!("borrow it instead: `&{name}`"))
        };
        self.emit(span, diagnostic);
    }

    fn report_immutable(
        &mut self,
        span: Span,
        place: &Place,
        access: Access,
        immutable: &places::Immutable,
    ) {
        let name = self.tys.describe(place);
        let action = match access {
            Access::Write => format!("cannot assign to `{name}`"),
            _ => format!("cannot borrow `{name}` as mutable"),
        };
        let through_this = immutable.through.projection.is_empty()
            && base_local(&immutable.through)
                .and_then(|local| self.body.local(local).name)
                .is_some_and(|name| name.as_str() == "this");
        let diagnostic = if immutable.is_static {
            Diagnostic::error(format!("{action}, because it is a static field"))
                .with_span(span)
                .with_note("a `static` field can be used from many threads at once")
        } else if self.tys.is_capture(place) {
            Diagnostic::error(format!("cannot change `{name}` inside a lambda"))
                .with_span(span)
                .with_label(format!("`{name}` is changed here"))
                .with_note("a lambda can only read the variables it captures")
        } else if through_this {
            Diagnostic::error(format!("{action} in a method that is not `mut`"))
                .with_span(span)
                .with_label("this changes the object")
                .with_help("mark the method `mut` to let it change `this`")
        } else {
            let holder = self.tys.describe(&immutable.through);
            Diagnostic::error(format!("{action}, because it is behind a `&` reference"))
                .with_span(span)
                .with_label(format!("`{holder}` only lets you read it"))
                .with_help(format!(
                    "borrow it with `&mut` to change it through `{holder}`"
                ))
        };
        self.emit(span, diagnostic);
    }

    fn report_conflict(
        &mut self,
        location: Location,
        span: Span,
        place: &Place,
        access: Access,
        index: usize,
        held: &Holdings,
    ) {
        let name = self.tys.describe(place);
        if !self.reported_places.insert((location, name.clone())) {
            return;
        }
        let loan = &self.loans.loans[index];
        let borrowed = self.tys.describe(&loan.place);
        let kind = if loan.mutable { "mutable" } else { "shared" };
        let (message, label) = match access {
            Access::Borrow { mutable: true } if loan.mutable => (
                format!("cannot borrow `{name}` as mutable more than once at a time"),
                "second mutable borrow happens here".to_owned(),
            ),
            Access::Borrow { mutable: true } => (
                format!("cannot borrow `{name}` as mutable because it is also borrowed as shared"),
                "mutable borrow happens here".to_owned(),
            ),
            Access::Borrow { mutable: false } => (
                format!("cannot borrow `{name}` because it is borrowed as mutable"),
                "shared borrow happens here".to_owned(),
            ),
            Access::Read => (
                format!("cannot use `{name}` because it is borrowed as mutable"),
                format!("`{name}` is used here"),
            ),
            Access::Write => (
                format!("cannot assign to `{name}` because it is borrowed"),
                format!("`{name}` is assigned here while it is borrowed"),
            ),
            Access::Move => (
                format!("cannot move out of `{name}` because it is borrowed"),
                format!("`{name}` is moved here while it is borrowed"),
            ),
        };
        let first = if loan.captured {
            format!("`{borrowed}` is borrowed by a lambda here")
        } else {
            format!("{kind} borrow of `{borrowed}` happens here")
        };
        let mut diagnostic = Diagnostic::error(message)
            .with_span(span)
            .with_label(label)
            .with_secondary(loan.span, first);
        let holders = self.holders(location, held, index);
        if let Some(later) = self
            .later_use(location, &holders)
            .filter(|later| *later != loan.span)
        {
            diagnostic = diagnostic.with_secondary(later, "the borrow is used later here");
        }
        if loan.captured {
            diagnostic = diagnostic.with_help("use a `move` lambda to give it its own copy");
        }
        self.emit(span, diagnostic);
    }

    fn holders(&self, location: Location, held: &Holdings, index: usize) -> BitSet {
        let mut holders = BitSet::new(self.body.locals.len());
        let defined = self.defined_at(location);
        for local in self.liveness.live_after(location).iter() {
            if Some(local) != defined && held[local].contains(index) {
                holders.insert(local);
            }
        }
        holders
    }

    fn later_use(&self, location: Location, holders: &BitSet) -> Option<Span> {
        let mut queue = VecDeque::from([(location.block, location.index + 1)]);
        let mut visited = HashSet::new();
        while let Some((block, start)) = queue.pop_front() {
            if start == 0 && !visited.insert(block) {
                continue;
            }
            let data = self.body.block(block);
            for statement in data.statements.iter().skip(start) {
                let mut used = Vec::new();
                match &statement.kind {
                    StatementKind::Assign(assign) => {
                        rvalue_locals_of(&assign.1, &mut |local| used.push(local));
                        if assign.0.as_local().is_none() {
                            place_locals_of(&assign.0, &mut |local| used.push(local));
                        }
                    }
                    StatementKind::FakeRead(place) => {
                        place_locals_of(place, &mut |local| used.push(local));
                    }
                    StatementKind::StorageLive(_) | StatementKind::StorageDead(_) => {}
                }
                if used.iter().any(|local| holders.contains(local.index())) {
                    return Some(statement.span);
                }
            }
            if start <= data.statements.len() {
                let kind = &data.terminator.kind;
                let mut used = Vec::new();
                terminator_input_locals_of(kind, &mut |local| used.push(local));
                if matches!(kind, TerminatorKind::Return) {
                    used.push(Local::RETURN);
                }
                if used.iter().any(|local| holders.contains(local.index())) {
                    return Some(data.terminator.span);
                }
            }
            for successor in data.terminator.kind.successors() {
                queue.push_back((successor, 0));
            }
        }
        None
    }

    fn report_dangling(&mut self, loan: usize, span: Span) {
        let loan = &self.loans.loans[loan];
        let name = self.tys.describe(&loan.place);
        let diagnostic = if loan.captured {
            Diagnostic::error(format!("a lambda that borrows `{name}` outlives it"))
                .with_span(loan.span)
                .with_label(format!("`{name}` is borrowed by the lambda here"))
                .with_secondary(span, format!("`{name}` stops existing here"))
                .with_help("use a `move` lambda to give it its own copy")
        } else {
            Diagnostic::error(format!("`{name}` does not live long enough"))
                .with_span(loan.span)
                .with_label("borrowed value does not live long enough")
                .with_secondary(
                    span,
                    format!("`{name}` stops existing here while still borrowed"),
                )
        };
        self.emit(loan.span, diagnostic);
    }

    fn report_escape(&mut self, loan: usize, span: Span, how: &str) {
        let loan = &self.loans.loans[loan];
        let name = self.tys.describe(&loan.place);
        let diagnostic = if loan.captured {
            Diagnostic::error(format!(
                "a {how} lambda borrows `{name}`, which belongs to this method"
            ))
            .with_span(loan.span)
            .with_label(format!("`{name}` is borrowed by the lambda here"))
            .with_secondary(span, format!("the lambda is {how} here"))
            .with_note(format!("`{name}` stops existing when the method ends"))
            .with_help("use a `move` lambda to give it its own copy")
        } else {
            Diagnostic::error(format!(
                "cannot let a reference to `{name}` escape the method"
            ))
            .with_span(loan.span)
            .with_label(format!("`{name}` is borrowed here"))
            .with_secondary(span, format!("the reference is {how} here"))
            .with_note(format!("`{name}` stops existing when the method ends"))
        };
        self.emit(loan.span, diagnostic);
    }
}

fn past(access: Access) -> &'static str {
    match access {
        Access::Borrow { .. } => "borrowed",
        Access::Write => "assigned",
        Access::Read | Access::Move => "used",
    }
}

fn path_place(path: &PlacePath) -> Place {
    let mut place = Place::local(path.local);
    for elem in &path.elems {
        place = place.project(match elem {
            places::PathElem::Field(owner, name) => PlaceElem::Field {
                owner: *owner,
                name: *name,
            },
            places::PathElem::Capture(index) => PlaceElem::Capture(*index),
        });
    }
    place
}

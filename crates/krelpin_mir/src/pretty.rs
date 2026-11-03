use std::fmt::Write;

use krelpin_ast::ast::BinOp;
use krelpin_hir::Dispatch;
use krelpin_typeck::{Callee, Tcx, Ty};

use crate::mir::{
    AggregateKind, BasicBlock, Body, BodyId, BodySource, Check, ClosureTarget, Constant, Local,
    Mir, Operand, Place, PlaceBase, PlaceElem, Rvalue, StatementKind, TerminatorKind, UnaryOp,
    Unwind,
};

pub fn print(mir: &Mir, bodies: impl IntoIterator<Item = BodyId>, tcx: &Tcx<'_>) -> String {
    let mut out = String::new();
    for id in bodies {
        let mut printer = Printer {
            mir,
            tcx,
            id,
            body: mir.body(id),
            out: String::new(),
        };
        printer.body();
        out.push_str(&printer.out);
        out.push('\n');
    }
    out
}

struct Printer<'a, 'tcx> {
    mir: &'a Mir,
    tcx: &'a Tcx<'tcx>,
    id: BodyId,
    body: &'a Body,
    out: String,
}

impl Printer<'_, '_> {
    fn body(&mut self) {
        let args: Vec<String> = self
            .body
            .args()
            .map(|local| format!("{local:?}: {}", self.body.local(local).ty))
            .collect();
        let header = self.header(self.id);
        let _ = writeln!(
            self.out,
            "{header}({}) -> {} {{",
            args.join(", "),
            self.body.local(Local::RETURN).ty
        );
        for capture in &self.body.captures {
            let mode = if capture.mutable { "&mut " } else { "&" };
            let _ = writeln!(
                self.out,
                "    capture {}: {mode}{};",
                capture.name, capture.ty
            );
        }
        for (index, decl) in self.body.locals.iter().enumerate() {
            let local = Local(index as u32);
            let is_arg = index >= 1 && index <= self.body.arg_count;
            if index > 0 && !is_arg {
                let _ = writeln!(self.out, "    let {local:?}: {};", decl.ty);
            }
        }
        for (index, decl) in self.body.locals.iter().enumerate() {
            if let Some(name) = decl.name {
                let _ = writeln!(self.out, "    debug {name} => {:?};", Local(index as u32));
            }
        }
        for (index, block) in self.body.blocks.iter().enumerate() {
            let _ = writeln!(self.out, "\n    {:?}: {{", BasicBlock(index as u32));
            for statement in &block.statements {
                let line = match &statement.kind {
                    StatementKind::Assign(assign) => {
                        let (place, rvalue) = assign.as_ref();
                        format!("{} = {};", self.place(place), self.rvalue(rvalue))
                    }
                    StatementKind::StorageLive(local) => format!("StorageLive({local:?});"),
                    StatementKind::StorageDead(local) => format!("StorageDead({local:?});"),
                };
                let _ = writeln!(self.out, "        {line}");
            }
            let terminator = self.terminator(&block.terminator.kind);
            let _ = writeln!(self.out, "        {terminator}");
            self.out.push_str("    }\n");
        }
        self.out.push_str("}\n");
    }

    fn header(&self, id: BodyId) -> String {
        let body = self.mir.body(id);
        match body.source {
            BodySource::Method => format!("fn {}", self.path(id)),
            BodySource::Constructor => format!("ctor {}", self.path(id)),
            BodySource::FieldInit => format!("field {}", self.path(id)),
            BodySource::Closure { .. } => format!("fn {}", self.path(id)),
        }
    }

    fn path(&self, id: BodyId) -> String {
        let body = self.mir.body(id);
        match body.source {
            BodySource::Closure { parent, index } => {
                format!("{}::closure#{index}", self.path(parent))
            }
            _ => format!("{}.{}", self.tcx.sig(body.owner).name.name, body.name),
        }
    }

    fn place(&self, place: &Place) -> String {
        let mut out = match place.base {
            PlaceBase::Local(local) => format!("{local:?}"),
            PlaceBase::Static { owner, name } => {
                format!("{}::{name}", self.tcx.sig(owner).name.name)
            }
        };
        for elem in &place.projection {
            out = match elem {
                PlaceElem::Deref => format!("(*{out})"),
                PlaceElem::Field { name, .. } => format!("{out}.{name}"),
                PlaceElem::Index(local) => format!("{out}[{local:?}]"),
                PlaceElem::Variant { def, index } => {
                    let variant = self.tcx.sig(*def).variants[*index].name.name;
                    format!("({out} as {variant})")
                }
                PlaceElem::Subtype(ty) => format!("({out} as {ty})"),
                PlaceElem::Capture(index) => {
                    let name = self.body.captures[*index].name;
                    format!("{out}.{name}")
                }
            };
        }
        out
    }

    fn operand(&self, operand: &Operand) -> String {
        match operand {
            Operand::Copy(place) => format!("copy {}", self.place(place)),
            Operand::Move(place) => format!("move {}", self.place(place)),
            Operand::Constant(constant) => match constant {
                Constant::Lit(lit) => format!("const {lit}"),
                Constant::Int { value, ty } => format!("const {value}_{}", Ty::Primitive(*ty)),
                Constant::Bool(value) => format!("const {value}"),
                Constant::Null => "const null".to_owned(),
            },
        }
    }

    fn operands(&self, operands: &[Operand]) -> String {
        operands
            .iter()
            .map(|operand| self.operand(operand))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn rvalue(&self, rvalue: &Rvalue) -> String {
        match rvalue {
            Rvalue::Use(operand) => self.operand(operand),
            Rvalue::Ref { mutable, place } => {
                let prefix = if *mutable { "&mut " } else { "&" };
                format!("{prefix}{}", self.place(place))
            }
            Rvalue::Binary(op, lhs, rhs) => format!(
                "{}({}, {})",
                bin_op_name(*op),
                self.operand(lhs),
                self.operand(rhs)
            ),
            Rvalue::Unary(op, operand) => {
                let name = match op {
                    UnaryOp::Neg => "Neg",
                    UnaryOp::Not => "Not",
                    UnaryOp::BitNot => "BitNot",
                };
                format!("{name}({})", self.operand(operand))
            }
            Rvalue::Cast(operand, ty) => format!("{} as {ty}", self.operand(operand)),
            Rvalue::Len(place) => format!("Len({})", self.place(place)),
            Rvalue::Discriminant(place) => format!("discriminant({})", self.place(place)),
            Rvalue::InstanceOf(place, ty) => format!("{} instanceof {ty}", self.place(place)),
            Rvalue::IsNull(place) => format!("IsNull({})", self.place(place)),
            Rvalue::New(ty) => format!("new {ty}"),
            Rvalue::NewArray { elem, len } => format!("new {elem}[{}]", self.operand(len)),
            Rvalue::Aggregate(kind, operands) => self.aggregate(kind, operands),
            Rvalue::Caught => "caught".to_owned(),
        }
    }

    fn aggregate(&self, kind: &AggregateKind, operands: &[Operand]) -> String {
        match kind {
            AggregateKind::Array(_) => format!("[{}]", self.operands(operands)),
            AggregateKind::Record { def, .. } => {
                let sig = self.tcx.sig(*def);
                let fields: Vec<String> = sig
                    .components
                    .iter()
                    .zip(operands)
                    .map(|(name, operand)| format!("{name}: {}", self.operand(operand)))
                    .collect();
                format!("{} {{ {} }}", sig.name.name, fields.join(", "))
            }
            AggregateKind::Variant { def, index, .. } => {
                let sig = self.tcx.sig(*def);
                let variant = sig.variants[*index].name.name;
                if operands.is_empty() {
                    format!("{}::{variant}", sig.name.name)
                } else {
                    format!("{}::{variant}({})", sig.name.name, self.operands(operands))
                }
            }
            AggregateKind::Closure(ClosureTarget::Lambda(id)) => {
                format!("{} [{}]", self.path(*id), self.operands(operands))
            }
            AggregateKind::Closure(ClosureTarget::MethodRef { callee, constructs }) => {
                let target = match callee {
                    Some(callee) if *constructs => {
                        format!("{}::new", self.tcx.sig(callee.method.owner).name.name)
                    }
                    Some(callee) => self.callee(callee),
                    None => "<unknown>".to_owned(),
                };
                format!("{target} [{}]", self.operands(operands))
            }
        }
    }

    fn callee(&self, callee: &Callee) -> String {
        let owner = self.tcx.sig(callee.method.owner).name.name;
        format!("{owner}::{}", callee.method.name.name)
    }

    fn terminator(&self, kind: &TerminatorKind) -> String {
        match kind {
            TerminatorKind::Goto { target } => format!("goto -> {target:?};"),
            TerminatorKind::SwitchInt {
                discr,
                targets,
                otherwise,
            } => {
                let mut arms: Vec<String> = targets
                    .iter()
                    .map(|(value, target)| format!("{value}: {target:?}"))
                    .collect();
                arms.push(format!("otherwise: {otherwise:?}"));
                format!(
                    "switchInt({}) -> [{}];",
                    self.operand(discr),
                    arms.join(", ")
                )
            }
            TerminatorKind::Return => "return;".to_owned(),
            TerminatorKind::Unreachable => "unreachable;".to_owned(),
            TerminatorKind::Call {
                callee,
                dispatch,
                args,
                dest,
                target,
                unwind,
            } => {
                let dest = dest
                    .as_ref()
                    .map_or_else(String::new, |dest| format!("{} = ", self.place(dest)));
                let prefix = match (dispatch, callee.method.receiver) {
                    (Dispatch::Static, Some(_)) => "static ",
                    _ => "",
                };
                format!(
                    "{dest}{prefix}{}({}) -> [return: {target:?}, {}];",
                    self.callee(callee),
                    self.operands(args),
                    unwind_target(*unwind)
                )
            }
            TerminatorKind::Throw { value, unwind } => {
                format!(
                    "throw({}) -> [{}];",
                    self.operand(value),
                    unwind_target(*unwind)
                )
            }
            TerminatorKind::Assert {
                check,
                target,
                unwind,
            } => {
                let check = match check {
                    Check::Bounds { index, len } => {
                        format!("{} < {}", self.operand(index), self.operand(len))
                    }
                    Check::NonZero(operand) => format!("{} != 0", self.operand(operand)),
                    Check::InstanceOf { place, ty } => {
                        format!("{} instanceof {ty}", self.place(place))
                    }
                };
                format!(
                    "assert({check}) -> [success: {target:?}, {}];",
                    unwind_target(*unwind)
                )
            }
        }
    }
}

fn unwind_target(unwind: Unwind) -> String {
    match unwind {
        Unwind::Resume => "unwind continue".to_owned(),
        Unwind::Handler(block) => format!("unwind: {block:?}"),
    }
}

fn bin_op_name(op: BinOp) -> &'static str {
    match op {
        BinOp::Or | BinOp::And | BinOp::Elvis => {
            unreachable!("`{}` is lowered to control flow", op.as_str())
        }
        BinOp::BitOr => "BitOr",
        BinOp::BitXor => "BitXor",
        BinOp::BitAnd => "BitAnd",
        BinOp::Eq => "Eq",
        BinOp::Ne => "Ne",
        BinOp::Lt => "Lt",
        BinOp::Le => "Le",
        BinOp::Gt => "Gt",
        BinOp::Ge => "Ge",
        BinOp::Shl => "Shl",
        BinOp::Shr => "Shr",
        BinOp::UShr => "UShr",
        BinOp::Add => "Add",
        BinOp::Sub => "Sub",
        BinOp::Mul => "Mul",
        BinOp::Div => "Div",
        BinOp::Rem => "Rem",
    }
}

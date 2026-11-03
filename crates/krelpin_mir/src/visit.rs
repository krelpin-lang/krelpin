use crate::mir::{
    BasicBlockData, Check, Local, Operand, Place, PlaceBase, PlaceElem, Rvalue, StatementKind,
    TerminatorKind,
};

pub fn for_each_local_mut(blocks: &mut [BasicBlockData], f: &mut impl FnMut(&mut Local)) {
    for block in blocks {
        for statement in &mut block.statements {
            match &mut statement.kind {
                StatementKind::Assign(assign) => {
                    let (place, rvalue) = assign.as_mut();
                    place_locals(place, f);
                    rvalue_locals(rvalue, f);
                }
                StatementKind::StorageLive(local) | StatementKind::StorageDead(local) => f(local),
            }
        }
        match &mut block.terminator.kind {
            TerminatorKind::Goto { .. } | TerminatorKind::Return | TerminatorKind::Unreachable => {}
            TerminatorKind::SwitchInt { discr, .. } => operand_locals(discr, f),
            TerminatorKind::Call { args, dest, .. } => {
                for arg in args {
                    operand_locals(arg, f);
                }
                if let Some(dest) = dest {
                    place_locals(dest, f);
                }
            }
            TerminatorKind::Throw { value, .. } => operand_locals(value, f),
            TerminatorKind::Assert { check, .. } => match check {
                Check::Bounds { index, len } => {
                    operand_locals(index, f);
                    operand_locals(len, f);
                }
                Check::NonZero(operand) => operand_locals(operand, f),
                Check::InstanceOf { place, .. } => place_locals(place, f),
            },
        }
    }
}

fn place_locals(place: &mut Place, f: &mut impl FnMut(&mut Local)) {
    if let PlaceBase::Local(local) = &mut place.base {
        f(local);
    }
    for elem in &mut place.projection {
        if let PlaceElem::Index(local) = elem {
            f(local);
        }
    }
}

fn operand_locals(operand: &mut Operand, f: &mut impl FnMut(&mut Local)) {
    match operand {
        Operand::Copy(place) | Operand::Move(place) => place_locals(place, f),
        Operand::Constant(_) => {}
    }
}

fn rvalue_locals(rvalue: &mut Rvalue, f: &mut impl FnMut(&mut Local)) {
    match rvalue {
        Rvalue::Use(operand)
        | Rvalue::Unary(_, operand)
        | Rvalue::Cast(operand, _)
        | Rvalue::NewArray { len: operand, .. } => operand_locals(operand, f),
        Rvalue::Binary(_, lhs, rhs) => {
            operand_locals(lhs, f);
            operand_locals(rhs, f);
        }
        Rvalue::Ref { place, .. }
        | Rvalue::Len(place)
        | Rvalue::Discriminant(place)
        | Rvalue::InstanceOf(place, _)
        | Rvalue::IsNull(place) => place_locals(place, f),
        Rvalue::Aggregate(_, operands) => {
            for operand in operands {
                operand_locals(operand, f);
            }
        }
        Rvalue::New(_) | Rvalue::Caught => {}
    }
}

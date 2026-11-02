use std::collections::HashMap;
use std::fmt;

use krelpin_ast::ast::PrimitiveTy;
use krelpin_span::Symbol;
use krelpin_span::def_id::DefId;

pub type Subst = HashMap<Symbol, Ty>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    Primitive(PrimitiveTy),
    Void,
    Null,
    Adt {
        def: DefId,
        name: Symbol,
        args: Vec<Ty>,
    },
    External {
        name: Symbol,
        args: Vec<Ty>,
    },
    Param(Symbol),
    Ref {
        mutable: bool,
        ty: Box<Ty>,
    },
    Nullable(Box<Ty>),
    Array(Box<Ty>),
    Unknown,
}

impl Ty {
    pub const BOOLEAN: Ty = Ty::Primitive(PrimitiveTy::Boolean);
    pub const INT: Ty = Ty::Primitive(PrimitiveTy::Int);
    pub const LONG: Ty = Ty::Primitive(PrimitiveTy::Long);
    pub const FLOAT: Ty = Ty::Primitive(PrimitiveTy::Float);
    pub const DOUBLE: Ty = Ty::Primitive(PrimitiveTy::Double);
    pub const CHAR: Ty = Ty::Primitive(PrimitiveTy::Char);

    pub fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    pub fn references_unknown(&self) -> bool {
        match self {
            Ty::Unknown => true,
            Ty::Adt { args, .. } | Ty::External { args, .. } => {
                args.iter().any(Ty::references_unknown)
            }
            Ty::Ref { ty, .. } | Ty::Nullable(ty) | Ty::Array(ty) => ty.references_unknown(),
            Ty::Primitive(_) | Ty::Void | Ty::Null | Ty::Param(_) => false,
        }
    }

    pub fn is_boolean(&self) -> bool {
        *self == Ty::BOOLEAN
    }

    pub fn primitive(&self) -> Option<PrimitiveTy> {
        match self {
            Ty::Primitive(primitive) => Some(*primitive),
            _ => None,
        }
    }

    pub fn is_numeric(&self) -> bool {
        self.primitive().is_some_and(is_numeric)
    }

    pub fn is_integral(&self) -> bool {
        self.primitive().is_some_and(is_integral)
    }

    pub fn strip_refs(&self) -> &Ty {
        match self {
            Ty::Ref { ty, .. } => ty.strip_refs(),
            ty => ty,
        }
    }

    pub fn non_null(&self) -> &Ty {
        match self {
            Ty::Nullable(ty) => ty,
            ty => ty,
        }
    }

    pub fn nullable(self) -> Ty {
        match self {
            Ty::Nullable(_) | Ty::Null | Ty::Unknown => self,
            ty => Ty::Nullable(Box::new(ty)),
        }
    }

    pub fn subst(&self, subst: &Subst) -> Ty {
        if subst.is_empty() {
            return self.clone();
        }
        let all = |args: &[Ty]| args.iter().map(|arg| arg.subst(subst)).collect();
        match self {
            Ty::Param(name) => subst.get(name).cloned().unwrap_or_else(|| self.clone()),
            Ty::Adt { def, name, args } => Ty::Adt {
                def: *def,
                name: *name,
                args: all(args),
            },
            Ty::External { name, args } => Ty::External {
                name: *name,
                args: all(args),
            },
            Ty::Ref { mutable, ty } => Ty::Ref {
                mutable: *mutable,
                ty: Box::new(ty.subst(subst)),
            },
            Ty::Nullable(ty) => Ty::Nullable(Box::new(ty.subst(subst))),
            Ty::Array(ty) => Ty::Array(Box::new(ty.subst(subst))),
            Ty::Primitive(_) | Ty::Void | Ty::Null | Ty::Unknown => self.clone(),
        }
    }
}

pub fn is_numeric(primitive: PrimitiveTy) -> bool {
    primitive != PrimitiveTy::Boolean
}

pub fn is_integral(primitive: PrimitiveTy) -> bool {
    matches!(
        primitive,
        PrimitiveTy::Byte
            | PrimitiveTy::Short
            | PrimitiveTy::Char
            | PrimitiveTy::Int
            | PrimitiveTy::Long
    )
}

pub fn promote(a: PrimitiveTy, b: PrimitiveTy) -> Option<Ty> {
    [
        PrimitiveTy::Int,
        PrimitiveTy::Long,
        PrimitiveTy::Float,
        PrimitiveTy::Double,
    ]
    .into_iter()
    .find(|&to| widens(a, to) && widens(b, to))
    .map(Ty::Primitive)
}

pub fn promote_unary(primitive: PrimitiveTy) -> Ty {
    match primitive {
        PrimitiveTy::Byte | PrimitiveTy::Short | PrimitiveTy::Char => Ty::INT,
        primitive => Ty::Primitive(primitive),
    }
}

pub fn widens(from: PrimitiveTy, to: PrimitiveTy) -> bool {
    use PrimitiveTy::*;
    from == to
        || matches!(
            (from, to),
            (Byte, Short | Int | Long | Float | Double)
                | (Short | Char, Int | Long | Float | Double)
                | (Int, Long | Double)
                | (Float, Double)
        )
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let with_args = |f: &mut fmt::Formatter<'_>, name: &Symbol, args: &[Ty]| {
            write!(f, "{name}")?;
            if !args.is_empty() {
                let args: Vec<String> = args.iter().map(Ty::to_string).collect();
                write!(f, "<{}>", args.join(", "))?;
            }
            Ok(())
        };

        match self {
            Ty::Primitive(primitive) => f.write_str(primitive.name()),
            Ty::Void => f.write_str("void"),
            Ty::Null => f.write_str("null"),
            Ty::Adt { name, args, .. } | Ty::External { name, args } => with_args(f, name, args),
            Ty::Param(name) => write!(f, "{name}"),
            Ty::Ref { mutable, ty } => {
                f.write_str(if *mutable { "&mut " } else { "&" })?;
                match ty.as_ref() {
                    Ty::Nullable(_) => write!(f, "({ty})"),
                    ty => write!(f, "{ty}"),
                }
            }
            Ty::Nullable(ty) => write!(f, "{ty}?"),
            Ty::Array(ty) => match ty.as_ref() {
                Ty::Ref { .. } | Ty::Nullable(_) => write!(f, "({ty})[]"),
                ty => write!(f, "{ty}[]"),
            },
            Ty::Unknown => f.write_str("{unknown}"),
        }
    }
}

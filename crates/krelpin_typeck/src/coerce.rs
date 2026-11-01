use krelpin_ast::ast::PrimitiveTy;

use crate::sig::Tcx;
use crate::ty::{Ty, boxed_name, promote, widens};

const PRELUDE_SUPERTYPES: &[(&str, &str)] = &[
    ("ArrayList", "List"),
    ("HashMap", "Map"),
    ("HashSet", "Set"),
    ("List", "Iterable"),
    ("Set", "Iterable"),
    ("RuntimeException", "Exception"),
    ("IOException", "Exception"),
];

impl Tcx<'_> {
    pub fn is_assignable(&self, from: &Ty, to: &Ty) -> bool {
        match (from, to) {
            (Ty::Unknown, _) | (_, Ty::Unknown) => true,
            _ if from == to => true,
            (Ty::Null, Ty::Nullable(_)) => true,
            (Ty::Nullable(from), Ty::Nullable(to)) => self.is_assignable(from, to),
            (Ty::Nullable(_) | Ty::Null, _) => false,
            (from, Ty::Nullable(to)) => self.is_assignable(from, to),
            (Ty::Primitive(from), Ty::Primitive(to)) => widens(*from, *to),
            (Ty::Primitive(primitive), Ty::Prelude { name, .. })
            | (Ty::Prelude { name, .. }, Ty::Primitive(primitive)) => {
                name.as_str() == boxed_name(*primitive)
            }
            (
                Ty::Ref {
                    mutable: from_mut,
                    ty: from,
                },
                Ty::Ref {
                    mutable: to_mut,
                    ty: to,
                },
            ) => {
                if *to_mut {
                    *from_mut && (from == to || from.is_unknown() || to.is_unknown())
                } else {
                    self.is_assignable(from, to)
                }
            }
            (Ty::Ref { .. }, _) | (_, Ty::Ref { .. }) => false,
            (Ty::Primitive(_) | Ty::Void, _) | (_, Ty::Primitive(_) | Ty::Void) => false,
            (_, Ty::Prelude { name, .. }) if name.as_str() == "Object" => true,
            (Ty::External { .. }, _) | (_, Ty::External { .. }) => true,
            (Ty::Array(from), Ty::Array(to)) => from == to || from.is_unknown() || to.is_unknown(),
            (Ty::Adt { def, args, .. }, to) => self.adt_assignable(*def, args, to),
            (
                Ty::Prelude {
                    name: from,
                    args: from_args,
                },
                Ty::Prelude {
                    name: to,
                    args: to_args,
                },
            ) => args_compatible(from_args, to_args) && prelude_subtype(from.as_str(), to.as_str()),
            _ => false,
        }
    }

    fn adt_assignable(&self, def: usize, args: &[Ty], to: &Ty) -> bool {
        match to {
            Ty::Adt {
                def: target,
                args: target_args,
                ..
            } if *target == def => return args_compatible(args, target_args),
            _ => {}
        }

        let subst = self.subst_for(def, args);
        let sig = &self.types[def];
        sig.superclass
            .iter()
            .chain(sig.interfaces.iter())
            .any(|parent| self.is_assignable(&parent.subst(&subst), to))
    }

    pub fn common_ty(&self, a: &Ty, b: &Ty) -> Option<Ty> {
        match (a, b) {
            (Ty::Null, other) | (other, Ty::Null) => Some(other.clone().nullable()),
            (Ty::Primitive(a), Ty::Primitive(b))
                if *a != PrimitiveTy::Boolean && *b != PrimitiveTy::Boolean =>
            {
                Some(promote(*a, *b))
            }
            _ if self.is_assignable(a, b) => Some(b.clone()),
            _ if self.is_assignable(b, a) => Some(a.clone()),
            _ => None,
        }
    }
}

fn args_compatible(a: &[Ty], b: &[Ty]) -> bool {
    a.is_empty()
        || b.is_empty()
        || (a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(a, b)| a == b || a.is_unknown() || b.is_unknown()))
}

fn prelude_subtype(from: &str, to: &str) -> bool {
    from == to
        || PRELUDE_SUPERTYPES
            .iter()
            .any(|&(sub, sup)| sub == from && prelude_subtype(sup, to))
}

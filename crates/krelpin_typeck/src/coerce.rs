use krelpin_ast::ast::PrimitiveTy;
use krelpin_span::def_id::DefId;

use crate::lang_items::LangItem;
use crate::sig::Tcx;
use crate::ty::{Ty, promote, widens};

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
            (Ty::Primitive(primitive), Ty::Adt { def, .. })
            | (Ty::Adt { def, .. }, Ty::Primitive(primitive)) => {
                self.lang_items.is(*def, LangItem::boxed(*primitive))
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
            (_, Ty::Adt { def, .. }) if self.lang_items.is(*def, LangItem::Object) => true,
            (Ty::External { .. }, _) | (_, Ty::External { .. }) => true,
            (Ty::Array(from), Ty::Array(to)) => from == to || from.is_unknown() || to.is_unknown(),
            (Ty::Adt { def, args, .. }, to) => self.adt_assignable(*def, args, to),
            _ => false,
        }
    }

    fn adt_assignable(&self, def: DefId, args: &[Ty], to: &Ty) -> bool {
        match to {
            Ty::Adt {
                def: target,
                args: target_args,
                ..
            } if *target == def => return args_compatible(args, target_args),
            _ => {}
        }

        let subst = self.subst_for(def, args);
        let sig = self.sig(def);
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

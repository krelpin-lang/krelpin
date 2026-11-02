use std::fmt;

use krelpin_ast::ast::PrimitiveTy;
use krelpin_span::Span;

use crate::sig::{AdtKind, Tcx};
use crate::ty::Ty;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Ctor {
    Wildcard,
    Variant(usize),
    Bool(bool),
    Opaque(Option<String>),
}

impl Ctor {
    fn covers(&self, other: &Ctor) -> bool {
        match (self, other) {
            (Ctor::Opaque(Some(this)), Ctor::Opaque(Some(other))) => this == other,
            (Ctor::Opaque(_), _) | (_, Ctor::Opaque(_)) => false,
            (this, other) => this == other,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DeconPat {
    ctor: Ctor,
    fields: Vec<DeconPat>,
}

impl DeconPat {
    pub(crate) fn wildcard() -> DeconPat {
        DeconPat::new(Ctor::Wildcard, Vec::new())
    }

    pub(crate) fn new(ctor: Ctor, fields: Vec<DeconPat>) -> DeconPat {
        DeconPat { ctor, fields }
    }
}

pub(crate) struct MatchArm {
    pub(crate) pat: DeconPat,
    pub(crate) span: Span,
    pub(crate) has_guard: bool,
    pub(crate) has_errors: bool,
}

pub(crate) struct MatchReport {
    pub(crate) unreachable: Vec<Span>,
    pub(crate) missing: Vec<Witness>,
}

#[derive(Clone, Debug)]
pub(crate) enum Witness {
    Wild,
    Ctor { name: String, fields: Vec<Witness> },
}

impl fmt::Display for Witness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Witness::Wild => f.write_str("_"),
            Witness::Ctor { name, fields } if fields.is_empty() => f.write_str(name),
            Witness::Ctor { name, fields } => {
                let fields: Vec<String> = fields.iter().map(Witness::to_string).collect();
                write!(f, "{name}({})", fields.join(", "))
            }
        }
    }
}

enum CtorSet {
    Variants(Vec<(String, Vec<Ty>)>),
    Bool,
    Infinite,
}

impl CtorSet {
    fn all(&self) -> Vec<Ctor> {
        match self {
            CtorSet::Variants(variants) => (0..variants.len()).map(Ctor::Variant).collect(),
            CtorSet::Bool => vec![Ctor::Bool(false), Ctor::Bool(true)],
            CtorSet::Infinite => Vec::new(),
        }
    }

    fn is_finite(&self) -> bool {
        !matches!(self, CtorSet::Infinite)
    }

    fn name(&self, ctor: &Ctor) -> String {
        match (self, ctor) {
            (CtorSet::Variants(variants), Ctor::Variant(index)) => variants[*index].0.clone(),
            (_, Ctor::Bool(value)) => value.to_string(),
            _ => "_".to_owned(),
        }
    }
}

type Row = Vec<DeconPat>;

pub(crate) struct MatchCheck<'a, 'tcx> {
    tcx: &'a Tcx<'tcx>,
}

impl<'a, 'tcx> MatchCheck<'a, 'tcx> {
    pub(crate) fn new(tcx: &'a Tcx<'tcx>) -> Self {
        MatchCheck { tcx }
    }

    pub(crate) fn analyze(&self, scrutinee: &Ty, arms: &[MatchArm]) -> MatchReport {
        let tys = vec![scrutinee.clone()];
        let mut rows: Vec<Row> = Vec::new();
        let mut unreachable = Vec::new();
        for arm in arms {
            let row = vec![arm.pat.clone()];
            if !self.is_useful(&rows, &row, &tys) {
                unreachable.push(arm.span);
            }
            if !arm.has_guard {
                rows.push(row);
            }
        }
        let missing = self
            .missing(&rows, &tys)
            .into_iter()
            .filter_map(|mut witness| witness.pop())
            .collect();
        MatchReport {
            unreachable,
            missing,
        }
    }

    fn ctor_set(&self, ty: &Ty) -> CtorSet {
        match ty.strip_refs() {
            Ty::Primitive(PrimitiveTy::Boolean) => CtorSet::Bool,
            Ty::Adt { def, args, .. } if self.tcx.sig(*def).kind == AdtKind::Enum => {
                let subst = self.tcx.subst_for(*def, args);
                let variants = self
                    .tcx
                    .sig(*def)
                    .variants
                    .iter()
                    .map(|variant| {
                        let fields = variant
                            .fields
                            .iter()
                            .map(|field| field.ty.subst(&subst))
                            .collect();
                        (variant.name.name.as_str().to_owned(), fields)
                    })
                    .collect();
                CtorSet::Variants(variants)
            }
            _ => CtorSet::Infinite,
        }
    }

    fn field_tys(&self, ty: &Ty, ctor: &Ctor) -> Vec<Ty> {
        match (self.ctor_set(ty), ctor) {
            (CtorSet::Variants(mut variants), Ctor::Variant(index)) if *index < variants.len() => {
                variants.swap_remove(*index).1
            }
            _ => Vec::new(),
        }
    }

    fn is_useful(&self, rows: &[Row], row: &[DeconPat], tys: &[Ty]) -> bool {
        let Some((head, rest)) = row.split_first() else {
            return rows.is_empty();
        };
        let ty = &tys[0];
        if head.ctor != Ctor::Wildcard {
            let field_tys = self.field_tys(ty, &head.ctor);
            let rows = specialize(rows, &head.ctor, field_tys.len());
            let row = concat(head.fields.clone(), rest);
            return self.is_useful(&rows, &row, &concat(field_tys, &tys[1..]));
        }

        let set = self.ctor_set(ty);
        if set.is_finite() && self.missing_ctors(&set, rows).is_empty() {
            return set.all().iter().any(|ctor| {
                let field_tys = self.field_tys(ty, ctor);
                let rows = specialize(rows, ctor, field_tys.len());
                let row = concat(vec![DeconPat::wildcard(); field_tys.len()], rest);
                self.is_useful(&rows, &row, &concat(field_tys, &tys[1..]))
            });
        }
        self.is_useful(&default_rows(rows), rest, &tys[1..])
    }

    fn missing(&self, rows: &[Row], tys: &[Ty]) -> Vec<Vec<Witness>> {
        let Some(ty) = tys.first() else {
            return if rows.is_empty() {
                vec![Vec::new()]
            } else {
                Vec::new()
            };
        };

        let set = self.ctor_set(ty);
        let missing_ctors = self.missing_ctors(&set, rows);
        if set.is_finite() && missing_ctors.is_empty() {
            let mut witnesses = Vec::new();
            for ctor in set.all() {
                let field_tys = self.field_tys(ty, &ctor);
                let arity = field_tys.len();
                let specialized = specialize(rows, &ctor, arity);
                for mut stack in self.missing(&specialized, &concat(field_tys, &tys[1..])) {
                    let fields = stack.drain(stack.len() - arity..).rev().collect();
                    stack.push(Witness::Ctor {
                        name: set.name(&ctor),
                        fields,
                    });
                    witnesses.push(stack);
                }
            }
            return witnesses;
        }

        let rest = self.missing(&default_rows(rows), &tys[1..]);
        let heads: Vec<Witness> = if set.is_finite() {
            missing_ctors
                .iter()
                .map(|ctor| Witness::Ctor {
                    name: set.name(ctor),
                    fields: self
                        .field_tys(ty, ctor)
                        .iter()
                        .map(|_| Witness::Wild)
                        .collect(),
                })
                .collect()
        } else {
            vec![Witness::Wild]
        };
        rest.iter()
            .flat_map(|stack| {
                heads
                    .iter()
                    .map(|head| concat(stack.clone(), std::slice::from_ref(head)))
            })
            .collect()
    }

    fn missing_ctors(&self, set: &CtorSet, rows: &[Row]) -> Vec<Ctor> {
        set.all()
            .into_iter()
            .filter(|ctor| !rows.iter().any(|row| row[0].ctor == *ctor))
            .collect()
    }
}

fn specialize(rows: &[Row], ctor: &Ctor, arity: usize) -> Vec<Row> {
    rows.iter()
        .filter_map(|row| {
            let (head, rest) = row.split_first()?;
            let fields = if head.ctor == Ctor::Wildcard {
                vec![DeconPat::wildcard(); arity]
            } else if head.ctor.covers(ctor) && head.fields.len() == arity {
                head.fields.clone()
            } else {
                return None;
            };
            Some(concat(fields, rest))
        })
        .collect()
}

fn default_rows(rows: &[Row]) -> Vec<Row> {
    rows.iter()
        .filter(|row| row[0].ctor == Ctor::Wildcard)
        .map(|row| row[1..].to_vec())
        .collect()
}

fn concat<T: Clone>(mut first: Vec<T>, rest: &[T]) -> Vec<T> {
    first.extend_from_slice(rest);
    first
}

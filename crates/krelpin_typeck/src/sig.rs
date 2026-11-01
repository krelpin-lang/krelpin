use std::collections::HashMap;

use krelpin_ast::ast::{
    self, CompilationUnit, FieldStorage, FnRetTy, GenericParamKind, Generics, Ident, ItemKind,
    Member, MemberKind, Param, ReceiverKind, TyKind, Visibility,
};
use krelpin_errors::DiagCtxt;
use krelpin_resolve::{Res, Resolutions};
use krelpin_span::Symbol;

use crate::ty::Ty;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdtKind {
    Class,
    Struct,
    Interface,
    Enum,
}

#[derive(Debug)]
pub struct TypeSig {
    pub name: Ident,
    pub kind: AdtKind,
    pub is_abstract: bool,
    pub generics: Vec<Symbol>,
    pub superclass: Option<Ty>,
    pub interfaces: Vec<Ty>,
    pub fields: HashMap<Symbol, FieldSig>,
    pub methods: HashMap<Symbol, Vec<MethodSig>>,
    pub ctors: Vec<MethodSig>,
    pub enum_constants: Vec<Symbol>,
}

#[derive(Clone, Debug)]
pub struct FieldSig {
    pub name: Ident,
    pub ty: Ty,
    pub vis: Visibility,
    pub storage: FieldStorage,
    pub owner: usize,
}

#[derive(Clone, Debug)]
pub struct MethodSig {
    pub name: Ident,
    pub params: Vec<Ty>,
    pub variadic: bool,
    pub ret: Ty,
    pub receiver: Option<ReceiverKind>,
    pub vis: Visibility,
    pub owner: usize,
    pub generics: Vec<Symbol>,
}

pub struct Tcx<'a> {
    pub types: Vec<TypeSig>,
    pub(crate) resolutions: &'a Resolutions,
    pub(crate) dcx: &'a DiagCtxt,
}

impl<'a> Tcx<'a> {
    pub fn new(unit: &CompilationUnit, resolutions: &'a Resolutions, dcx: &'a DiagCtxt) -> Self {
        let mut tcx = Tcx {
            types: Vec::new(),
            resolutions,
            dcx,
        };
        tcx.types = unit
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| tcx.collect_item(index, item))
            .collect();
        tcx
    }

    pub fn lower_ty(&self, ty: &ast::Ty) -> Ty {
        match &ty.kind {
            TyKind::Primitive(primitive) => Ty::Primitive(*primitive),
            TyKind::Path { path, args } => {
                let args: Vec<Ty> = args.iter().map(|arg| self.lower_ty(arg)).collect();
                let last = path.segments.last().map(|segment| segment.name);
                let [segment] = path.segments.as_slice() else {
                    return last.map_or(Ty::Unknown, |name| Ty::External { name, args });
                };
                match self.resolutions.get(segment.span) {
                    Some(Res::Item(def)) => Ty::Adt {
                        def,
                        name: segment.name,
                        args,
                    },
                    Some(Res::Prelude(name)) => Ty::Prelude { name, args },
                    Some(Res::Import(_)) => Ty::External {
                        name: segment.name,
                        args,
                    },
                    Some(Res::TypeParam(_)) => Ty::Param(segment.name),
                    _ => Ty::Unknown,
                }
            }
            TyKind::Ref { mutable, ty, .. } => Ty::Ref {
                mutable: *mutable,
                ty: Box::new(self.lower_ty(ty)),
            },
            TyKind::Nullable(ty) => Ty::Nullable(Box::new(self.lower_ty(ty))),
            TyKind::Array(ty) => Ty::Array(Box::new(self.lower_ty(ty))),
        }
    }

    pub fn self_ty(&self, def: usize) -> Ty {
        let sig = &self.types[def];
        Ty::Adt {
            def,
            name: sig.name.name,
            args: sig.generics.iter().map(|&name| Ty::Param(name)).collect(),
        }
    }

    pub fn subst_for(&self, def: usize, args: &[Ty]) -> crate::ty::Subst {
        let generics = &self.types[def].generics;
        if args.len() != generics.len() {
            return crate::ty::Subst::new();
        }
        generics.iter().copied().zip(args.iter().cloned()).collect()
    }

    pub fn is_subclass(&self, sub: usize, sup: usize) -> bool {
        if sub == sup {
            return true;
        }
        let sig = &self.types[sub];
        sig.superclass
            .iter()
            .chain(sig.interfaces.iter())
            .any(|parent| matches!(parent, Ty::Adt { def, .. } if self.is_subclass(*def, sup)))
    }

    fn collect_item(&self, index: usize, item: &ast::Item) -> TypeSig {
        let (kind, generics, superclass, interfaces, members): (_, _, _, Vec<Ty>, &[Member]) =
            match &item.kind {
                ItemKind::Class(class) => (
                    AdtKind::Class,
                    &class.generics,
                    class.extends.as_ref().map(|ty| self.lower_ty(ty)),
                    self.lower_tys(&class.implements),
                    &class.members,
                ),
                ItemKind::Struct(def) => (
                    AdtKind::Struct,
                    &def.generics,
                    None,
                    self.lower_tys(&def.implements),
                    &def.members,
                ),
                ItemKind::Interface(interface) => (
                    AdtKind::Interface,
                    &interface.generics,
                    None,
                    self.lower_tys(&interface.extends),
                    &interface.members,
                ),
                ItemKind::Enum(def) => (
                    AdtKind::Enum,
                    &EMPTY_GENERICS,
                    None,
                    self.lower_tys(&def.implements),
                    &def.members,
                ),
            };

        let mut sig = TypeSig {
            name: item.name,
            kind,
            is_abstract: kind == AdtKind::Interface || item.inheritance.abstract_kw.is_some(),
            generics: type_params(generics),
            superclass,
            interfaces,
            fields: HashMap::new(),
            methods: HashMap::new(),
            ctors: Vec::new(),
            enum_constants: Vec::new(),
        };

        if let ItemKind::Enum(def) = &item.kind {
            let self_ty = Ty::Adt {
                def: index,
                name: item.name.name,
                args: Vec::new(),
            };
            for constant in &def.constants {
                sig.enum_constants.push(constant.name.name);
                sig.fields.insert(
                    constant.name.name,
                    FieldSig {
                        name: constant.name,
                        ty: self_ty.clone(),
                        vis: Visibility::Public(constant.name.span),
                        storage: FieldStorage::Const(constant.name.span),
                        owner: index,
                    },
                );
            }
        }

        for member in members {
            let vis = if kind == AdtKind::Interface {
                Visibility::Public(member.span)
            } else {
                member.vis
            };
            match &member.kind {
                MemberKind::Field(field) => {
                    sig.fields.entry(field.name.name).or_insert(FieldSig {
                        name: field.name,
                        ty: self.lower_ty(&field.ty),
                        vis,
                        storage: field.storage,
                        owner: index,
                    });
                }
                MemberKind::Constructor(ctor) => {
                    sig.ctors.push(self.method_sig(
                        ctor.name,
                        &EMPTY_GENERICS,
                        &ctor.params,
                        Ty::Void,
                        None,
                        vis,
                        index,
                    ));
                }
                MemberKind::Method(method) => {
                    let ret = match &method.ret {
                        FnRetTy::Void(_) => Ty::Void,
                        FnRetTy::Ty(ty) => self.lower_ty(ty),
                    };
                    let method_sig = self.method_sig(
                        method.name,
                        &method.generics,
                        &method.params,
                        ret,
                        method.receiver.map(|receiver| receiver.kind),
                        vis,
                        index,
                    );
                    sig.methods
                        .entry(method.name.name)
                        .or_default()
                        .push(method_sig);
                }
            }
        }

        sig
    }

    #[allow(clippy::too_many_arguments)]
    fn method_sig(
        &self,
        name: Ident,
        generics: &Generics,
        params: &[Param],
        ret: Ty,
        receiver: Option<ReceiverKind>,
        vis: Visibility,
        owner: usize,
    ) -> MethodSig {
        MethodSig {
            name,
            params: params
                .iter()
                .map(|param| {
                    let ty = self.lower_ty(&param.ty);
                    if param.variadic {
                        Ty::Array(Box::new(ty))
                    } else {
                        ty
                    }
                })
                .collect(),
            variadic: params.last().is_some_and(|param| param.variadic),
            ret,
            receiver,
            vis,
            owner,
            generics: type_params(generics),
        }
    }

    fn lower_tys(&self, tys: &[ast::Ty]) -> Vec<Ty> {
        tys.iter().map(|ty| self.lower_ty(ty)).collect()
    }
}

static EMPTY_GENERICS: Generics = Generics {
    params: Vec::new(),
    span: krelpin_span::Span::DUMMY,
};

fn type_params(generics: &Generics) -> Vec<Symbol> {
    generics
        .params
        .iter()
        .filter(|param| matches!(param.kind, GenericParamKind::Type { .. }))
        .map(|param| param.name.name)
        .collect()
}

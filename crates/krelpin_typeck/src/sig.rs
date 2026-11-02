use std::collections::HashMap;

use krelpin_ast::ast::{
    self, CompilationUnit, FieldStorage, FnRetTy, GenericParamKind, Generics, Ident, ItemKind,
    Member, MemberKind, Param, ReceiverKind, TyKind, Visibility,
};
use krelpin_errors::DiagCtxt;
use krelpin_resolve::{ItemTable, Res, Resolutions, ResolverOutput};
use krelpin_span::Symbol;
use krelpin_span::def_id::DefId;

use crate::lang_items::{LangItem, LangItems};
use crate::ty::{Subst, Ty};

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
    pub owner: DefId,
}

#[derive(Clone, Debug)]
pub struct MethodSig {
    pub name: Ident,
    pub params: Vec<Ty>,
    pub variadic: bool,
    pub ret: Ty,
    pub receiver: Option<ReceiverKind>,
    pub vis: Visibility,
    pub owner: DefId,
    pub generics: Vec<Symbol>,
}

pub struct Tcx<'a> {
    pub types: Vec<TypeSig>,
    pub lang_items: LangItems,
    pub(crate) items: &'a ItemTable,
    pub(crate) resolutions: &'a Resolutions,
    pub(crate) dcx: &'a DiagCtxt,
}

impl<'a> Tcx<'a> {
    pub fn new(units: &[CompilationUnit], resolved: &'a ResolverOutput, dcx: &'a DiagCtxt) -> Self {
        let mut tcx = Tcx {
            types: Vec::new(),
            lang_items: LangItems::collect(&resolved.items, dcx),
            items: &resolved.items,
            resolutions: &resolved.resolutions,
            dcx,
        };
        tcx.types = resolved
            .items
            .items(units)
            .map(|(type_def, item)| tcx.collect_item(type_def.def, item))
            .collect();
        tcx
    }

    pub fn sig(&self, def: DefId) -> &TypeSig {
        &self.types[def.index()]
    }

    pub fn lang_ty(&self, item: LangItem) -> Ty {
        self.lang_items
            .get(item)
            .map_or(Ty::Unknown, |def| self.def_ty(def, Vec::new()))
    }

    pub fn is_lang_ty(&self, ty: &Ty, item: LangItem) -> bool {
        matches!(ty, Ty::Adt { def, .. } if self.lang_items.is(*def, item))
    }

    pub fn string_ty(&self) -> Ty {
        self.lang_ty(LangItem::String)
    }

    pub fn is_string(&self, ty: &Ty) -> bool {
        self.is_lang_ty(ty, LangItem::String)
    }

    fn def_ty(&self, def: DefId, args: Vec<Ty>) -> Ty {
        Ty::Adt {
            def,
            name: self.items.def(def).name.name,
            args,
        }
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
                    Some(Res::Item(def)) => self.def_ty(def, args),
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

    pub fn self_ty(&self, def: DefId) -> Ty {
        let generics = &self.sig(def).generics;
        self.def_ty(def, generics.iter().map(|&name| Ty::Param(name)).collect())
    }

    pub fn subst_for(&self, def: DefId, args: &[Ty]) -> Subst {
        let generics = &self.sig(def).generics;
        if args.len() == generics.len() {
            generics.iter().copied().zip(args.iter().cloned()).collect()
        } else {
            generics.iter().map(|&name| (name, Ty::Unknown)).collect()
        }
    }

    pub fn is_subclass(&self, sub: DefId, sup: DefId) -> bool {
        if sub == sup {
            return true;
        }
        let sig = self.sig(sub);
        sig.superclass
            .iter()
            .chain(sig.interfaces.iter())
            .any(|parent| matches!(parent, Ty::Adt { def, .. } if self.is_subclass(*def, sup)))
    }

    pub fn upcast(&self, def: DefId, args: &[Ty], target: DefId) -> Option<Vec<Ty>> {
        if def == target {
            return Some(args.to_vec());
        }
        let subst = self.subst_for(def, args);
        let sig = self.sig(def);
        sig.superclass
            .iter()
            .chain(sig.interfaces.iter())
            .find_map(|parent| match parent.subst(&subst) {
                Ty::Adt { def, args, .. } => self.upcast(def, &args, target),
                _ => None,
            })
    }

    fn implicit_superclass(&self, def: DefId, kind: AdtKind) -> Option<Ty> {
        let parent = match kind {
            AdtKind::Class | AdtKind::Struct => LangItem::Object,
            AdtKind::Enum => LangItem::Enum,
            AdtKind::Interface => return None,
        };
        let parent = self.lang_items.get(parent)?;
        (parent != def).then(|| self.def_ty(parent, Vec::new()))
    }

    fn collect_item(&self, def_id: DefId, item: &ast::Item) -> TypeSig {
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
            superclass: superclass.or_else(|| self.implicit_superclass(def_id, kind)),
            interfaces,
            fields: HashMap::new(),
            methods: HashMap::new(),
            ctors: Vec::new(),
            enum_constants: Vec::new(),
        };

        if let ItemKind::Enum(def) = &item.kind {
            let self_ty = self.def_ty(def_id, Vec::new());
            for constant in &def.constants {
                sig.enum_constants.push(constant.name.name);
                sig.fields.insert(
                    constant.name.name,
                    FieldSig {
                        name: constant.name,
                        ty: self_ty.clone(),
                        vis: Visibility::Public(constant.name.span),
                        storage: FieldStorage::Const(constant.name.span),
                        owner: def_id,
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
                        owner: def_id,
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
                        def_id,
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
                        def_id,
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
        owner: DefId,
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

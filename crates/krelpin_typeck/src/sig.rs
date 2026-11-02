use std::collections::HashMap;

use krelpin_ast::ast::{
    self, CompilationUnit, Constructor, FieldStorage, FnRetTy, GenericParamKind, Generics, Ident,
    ItemKind, Member, MemberKind, Method, Param, ReceiverKind, TyKind, Visibility, WherePredicate,
    is_intrinsic,
};
use krelpin_errors::DiagCtxt;
use krelpin_resolve::{ItemTable, Res, Resolutions, ResolverOutput};
use krelpin_span::Symbol;
use krelpin_span::def_id::DefId;

use crate::lang_items::{LangItem, LangItems};
use crate::param_env::ParamEnv;
use crate::ty::{Subst, Ty};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdtKind {
    Class,
    Record,
    Interface,
    Enum,
}

#[derive(Debug)]
pub struct TypeSig {
    pub name: Ident,
    pub kind: AdtKind,
    pub is_abstract: bool,
    pub is_final: bool,
    pub generics: Vec<GenericParamSig>,
    pub superclass: Option<Ty>,
    pub interfaces: Vec<Ty>,
    pub fields: HashMap<Symbol, FieldSig>,
    pub methods: HashMap<Symbol, Vec<MethodSig>>,
    pub ctors: Vec<MethodSig>,
    pub variants: Vec<VariantSig>,
    pub components: Vec<Symbol>,
}

impl TypeSig {
    pub fn variant(&self, name: Symbol) -> Option<&VariantSig> {
        self.variants
            .iter()
            .find(|variant| variant.name.name == name)
    }
}

#[derive(Clone, Debug)]
pub struct VariantSig {
    pub name: Ident,
    pub fields: Vec<VariantFieldSig>,
}

#[derive(Clone, Debug)]
pub struct VariantFieldSig {
    pub name: Ident,
    pub ty: Ty,
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
    pub generics: Vec<GenericParamSig>,
    pub where_bounds: Vec<GenericParamSig>,
    pub throws: Vec<Ty>,
    pub is_abstract: bool,
    pub is_final: bool,
}

#[derive(Clone, Debug)]
pub struct GenericParamSig {
    pub name: Symbol,
    pub bounds: Vec<Ty>,
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
                let Some(segment) = path.segments.last() else {
                    return Ty::Unknown;
                };
                match self.resolutions.get(segment.span) {
                    Some(Res::Item(def)) => self.def_ty(def, args),
                    Some(Res::Import(_)) => Ty::External {
                        name: segment.name,
                        args,
                    },
                    Some(Res::TypeParam(_)) => Ty::Param(segment.name),
                    None if path.segments.len() > 1 => Ty::External {
                        name: segment.name,
                        args,
                    },
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
        if let Some(primitive) = self.lang_items.primitive_of(def) {
            return Ty::Primitive(primitive);
        }
        let generics = &self.sig(def).generics;
        self.def_ty(
            def,
            generics.iter().map(|param| Ty::Param(param.name)).collect(),
        )
    }

    pub fn subst_for(&self, def: DefId, args: &[Ty]) -> Subst {
        let generics = &self.sig(def).generics;
        if args.len() == generics.len() {
            generics
                .iter()
                .map(|param| param.name)
                .zip(args.iter().cloned())
                .collect()
        } else {
            generics
                .iter()
                .map(|param| (param.name, Ty::Unknown))
                .collect()
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

    pub(crate) fn methods_named(
        &self,
        def: DefId,
        args: &[Ty],
        name: Symbol,
    ) -> Vec<(MethodSig, Subst)> {
        let mut out = Vec::new();
        self.find_methods(def, args, name, &mut out);
        if !out.is_empty() || self.sig(def).kind != AdtKind::Interface {
            return out;
        }
        if let Some(object) = self.lang_items.get(LangItem::Object) {
            self.find_methods(object, &[], name, &mut out);
        }
        out
    }

    fn find_methods(
        &self,
        def: DefId,
        args: &[Ty],
        name: Symbol,
        out: &mut Vec<(MethodSig, Subst)>,
    ) {
        let sig = self.sig(def);
        let subst = self.subst_for(def, args);
        if let Some(methods) = sig.methods.get(&name) {
            out.extend(methods.iter().map(|method| (method.clone(), subst.clone())));
        }
        for parent in sig.superclass.iter().chain(sig.interfaces.iter()) {
            if let Ty::Adt { def, args, .. } = parent.subst(&subst) {
                self.find_methods(def, &args, name, out);
            }
        }
    }

    pub(crate) fn supertypes(&self, def: DefId) -> Vec<(DefId, Subst)> {
        let mut out = Vec::new();
        let self_args: Vec<Ty> = self
            .sig(def)
            .generics
            .iter()
            .map(|param| Ty::Param(param.name))
            .collect();
        self.collect_supertypes(def, &self_args, &mut out);
        out
    }

    fn collect_supertypes(&self, def: DefId, args: &[Ty], out: &mut Vec<(DefId, Subst)>) {
        let subst = self.subst_for(def, args);
        let sig = self.sig(def);
        for parent in sig.superclass.iter().chain(sig.interfaces.iter()) {
            match parent.subst(&subst) {
                Ty::Adt { def, args, .. } if out.iter().all(|(seen, _)| *seen != def) => {
                    out.push((def, self.subst_for(def, &args)));
                    self.collect_supertypes(def, &args, out);
                }
                _ => {}
            }
        }
    }

    fn implicit_superclass(&self, def: DefId, kind: AdtKind) -> Option<Ty> {
        let parent = match kind {
            AdtKind::Class | AdtKind::Record => LangItem::Object,
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
                ItemKind::Record(def) => (
                    AdtKind::Record,
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
                    &def.generics,
                    None,
                    self.lower_tys(&def.implements),
                    &def.members,
                ),
            };

        let mut sig = TypeSig {
            name: item.name,
            kind,
            is_abstract: kind == AdtKind::Interface || item.inheritance.abstract_kw.is_some(),
            is_final: item.inheritance.final_kw.is_some(),
            generics: self.generic_params(generics),
            superclass: superclass.or_else(|| self.implicit_superclass(def_id, kind)),
            interfaces,
            fields: HashMap::new(),
            methods: HashMap::new(),
            ctors: Vec::new(),
            variants: Vec::new(),
            components: Vec::new(),
        };

        if let ItemKind::Enum(def) = &item.kind {
            self.collect_variants(&mut sig, def_id, &def.variants);
        }
        if let ItemKind::Record(def) = &item.kind {
            self.collect_components(&mut sig, def_id, def);
        }

        for member in members {
            let vis = match member.vis {
                _ if kind == AdtKind::Interface => Visibility::Public(member.span),
                vis => vis,
            };
            match &member.kind {
                MemberKind::Type(_) => {}
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
                    sig.ctors.push(self.ctor_sig(ctor, vis, def_id));
                }
                MemberKind::Method(method) => {
                    let method_sig = self.method_sig(member, method, vis, def_id);
                    sig.methods
                        .entry(method.name.name)
                        .or_default()
                        .push(method_sig);
                }
            }
        }

        sig
    }

    fn collect_components(&self, sig: &mut TypeSig, def: DefId, record: &ast::Record) {
        for component in &record.components {
            let name = component.name;
            sig.components.push(name.name);
            sig.fields.entry(name.name).or_insert(FieldSig {
                name,
                ty: self.lower_ty(&component.ty),
                vis: Visibility::Public(name.span),
                storage: FieldStorage::Instance,
                owner: def,
            });
        }
        let has_compact_ctor = record
            .members
            .iter()
            .any(|member| matches!(&member.kind, MemberKind::Constructor(ctor) if ctor.compact));
        if !has_compact_ctor {
            sig.ctors.push(MethodSig {
                params: self.param_tys(&record.components),
                is_final: false,
                ..synthesized_method(sig.name, def, None, Ty::Void)
            });
        }
    }

    fn collect_variants(&self, sig: &mut TypeSig, def: DefId, variants: &[ast::Variant]) {
        let self_ty = self.def_ty(
            def,
            sig.generics
                .iter()
                .map(|param| Ty::Param(param.name))
                .collect(),
        );
        for variant in variants {
            let fields: Vec<VariantFieldSig> = variant
                .fields
                .iter()
                .map(|field| VariantFieldSig {
                    name: field.name,
                    ty: self.lower_ty(&field.ty),
                })
                .collect();
            let name = variant.name;
            if fields.is_empty() {
                sig.fields.entry(name.name).or_insert(FieldSig {
                    name,
                    ty: self_ty.clone(),
                    vis: Visibility::Public(name.span),
                    storage: FieldStorage::Const(name.span),
                    owner: def,
                });
            } else {
                let ctor = MethodSig {
                    params: fields.iter().map(|field| field.ty.clone()).collect(),
                    generics: sig.generics.clone(),
                    ..synthesized_method(name, def, None, self_ty.clone())
                };
                sig.methods.entry(name.name).or_default().push(ctor);
            }
            sig.variants.push(VariantSig { name, fields });
        }

        if sig.variants.iter().all(|variant| variant.fields.is_empty()) {
            self.synthesize_unit_enum_members(sig, def, self_ty);
        }
    }

    pub(crate) fn derive_record_equality(&mut self) {
        loop {
            let mut changed = false;
            for def in (0..self.types.len()).map(DefId::from_index) {
                for item in [LangItem::Equatable, LangItem::Hashable] {
                    changed |= self.add_record_interface(def, item);
                }
            }
            if !changed {
                return;
            }
        }
    }

    fn add_record_interface(&mut self, def: DefId, item: LangItem) -> bool {
        let sig = self.sig(def);
        let Some(interface) = self.lang_items.get(item) else {
            return false;
        };
        let method_name = match item {
            LangItem::Equatable => "equals",
            _ => "hashCode",
        };
        let already_implemented = sig.methods.contains_key(&Symbol::intern(method_name))
            || sig.interfaces.iter().any(
                |parent| matches!(parent, Ty::Adt { def: parent, .. } if *parent == interface),
            );
        if sig.kind != AdtKind::Record || already_implemented {
            return false;
        }
        let env = ParamEnv::default().with_generics(&sig.generics);
        let every_component = sig.components.iter().all(|name| {
            let ty = sig.fields[name].ty.clone();
            let bound = self.def_ty(interface, vec![ty.clone()]);
            ty.references_unknown() || self.is_assignable(&env, &ty, &bound)
        });
        if !every_component {
            return false;
        }

        let self_ty = self.self_ty(def);
        let span = sig.name.span;
        let name = Ident::new(Symbol::intern(method_name), span);
        let method = match item {
            LangItem::Equatable => MethodSig {
                params: vec![Ty::Ref {
                    mutable: false,
                    ty: Box::new(self_ty.clone()),
                }],
                ..synthesized_method(name, def, Some(ReceiverKind::Ref), Ty::BOOLEAN)
            },
            _ => synthesized_method(name, def, Some(ReceiverKind::Ref), Ty::INT),
        };
        let interface_ty = self.def_ty(interface, vec![self_ty]);
        let sig = &mut self.types[def.index()];
        sig.interfaces.push(interface_ty);
        sig.methods
            .entry(method.name.name)
            .or_default()
            .push(method);
        true
    }

    fn synthesize_unit_enum_members(&self, sig: &mut TypeSig, def: DefId, self_ty: Ty) {
        if let Some(hashable) = self.lang_items.get(LangItem::Hashable) {
            sig.interfaces
                .push(self.def_ty(hashable, vec![self_ty.clone()]));
        }

        let span = sig.name.span;
        let name = |name: &str| Ident::new(Symbol::intern(name), span);
        let borrowed_self = Ty::Ref {
            mutable: false,
            ty: Box::new(self_ty.clone()),
        };
        let methods = [
            synthesized_method(name("values"), def, None, Ty::Array(Box::new(self_ty))),
            MethodSig {
                params: vec![borrowed_self],
                ..synthesized_method(name("equals"), def, Some(ReceiverKind::Ref), Ty::BOOLEAN)
            },
            synthesized_method(name("hashCode"), def, Some(ReceiverKind::Ref), Ty::INT),
        ];
        for method in methods {
            sig.methods
                .entry(method.name.name)
                .or_default()
                .push(method);
        }
    }

    fn method_sig(
        &self,
        member: &Member,
        method: &Method,
        vis: Visibility,
        owner: DefId,
    ) -> MethodSig {
        let ret = match &method.ret {
            FnRetTy::Void(_) => Ty::Void,
            FnRetTy::Ty(ty) => self.lower_ty(ty),
        };
        let is_abstract = member.inheritance.abstract_kw.is_some()
            || (method.body.is_none() && !is_intrinsic(&member.annotations));
        MethodSig {
            name: method.name,
            params: self.param_tys(&method.params),
            variadic: is_variadic(&method.params),
            ret,
            receiver: method.receiver.map(|receiver| receiver.kind),
            vis,
            owner,
            generics: self.generic_params(&method.generics),
            where_bounds: self.where_bounds(&method.where_clause),
            throws: self.lower_tys(&method.throws),
            is_abstract,
            is_final: member.inheritance.final_kw.is_some(),
        }
    }

    fn ctor_sig(&self, ctor: &Constructor, vis: Visibility, owner: DefId) -> MethodSig {
        MethodSig {
            name: ctor.name,
            params: self.param_tys(&ctor.params),
            variadic: is_variadic(&ctor.params),
            ret: Ty::Void,
            receiver: None,
            vis,
            owner,
            generics: Vec::new(),
            where_bounds: Vec::new(),
            throws: self.lower_tys(&ctor.throws),
            is_abstract: false,
            is_final: false,
        }
    }

    fn param_tys(&self, params: &[Param]) -> Vec<Ty> {
        params
            .iter()
            .map(|param| {
                let ty = self.lower_ty(&param.ty);
                if param.variadic {
                    Ty::Array(Box::new(ty))
                } else {
                    ty
                }
            })
            .collect()
    }

    pub(crate) fn where_bounds(&self, predicates: &[WherePredicate]) -> Vec<GenericParamSig> {
        predicates
            .iter()
            .map(|predicate| GenericParamSig {
                name: predicate.param.name,
                bounds: self.lower_tys(&predicate.bounds),
            })
            .collect()
    }

    pub(crate) fn generic_params(&self, generics: &Generics) -> Vec<GenericParamSig> {
        generics
            .params
            .iter()
            .filter_map(|param| match &param.kind {
                GenericParamKind::Type { bounds } => Some(GenericParamSig {
                    name: param.name.name,
                    bounds: self.lower_tys(bounds),
                }),
                GenericParamKind::BorrowSource => None,
            })
            .collect()
    }

    pub(crate) fn lower_tys(&self, tys: &[ast::Ty]) -> Vec<Ty> {
        tys.iter().map(|ty| self.lower_ty(ty)).collect()
    }
}

fn is_variadic(params: &[Param]) -> bool {
    params.last().is_some_and(|param| param.variadic)
}

fn synthesized_method(
    name: Ident,
    owner: DefId,
    receiver: Option<ReceiverKind>,
    ret: Ty,
) -> MethodSig {
    MethodSig {
        name,
        params: Vec::new(),
        variadic: false,
        ret,
        receiver,
        vis: Visibility::Public(name.span),
        owner,
        generics: Vec::new(),
        where_bounds: Vec::new(),
        throws: Vec::new(),
        is_abstract: false,
        is_final: true,
    }
}

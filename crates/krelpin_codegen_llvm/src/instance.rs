use krelpin_ast::ast::ReceiverKind;
use krelpin_mir::{BodyId, BodySource};
use krelpin_span::Symbol;
use krelpin_span::def_id::DefId;
use krelpin_typeck::{AdtKind, LangItem, MethodSig, Subst, Ty};

use crate::cx::Cx;
use crate::entry::method_of;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Instance {
    Body {
        body: BodyId,
        args: Vec<Ty>,
    },
    Builtin {
        owner: DefId,
        name: Symbol,
        member: Option<u32>,
        args: Vec<Ty>,
        self_ty: Ty,
    },
    Thunk(Thunk),
    /// The `poll` of the future of an `async` body; see `asyncness.rs`.
    AsyncPoll {
        body: BodyId,
        args: Vec<Ty>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Thunk {
    Boxed {
        value: Ty,
        selector: usize,
        target: Box<Instance>,
    },
    Lambda {
        closure: usize,
        selector: usize,
    },
    MethodRef {
        closure: usize,
        selector: usize,
    },
    ThreadEntry {
        runnable: Ty,
    },
    TableHash(Ty),
    TableEq(Ty),
    TableDrop(Ty),
    DropObject(Ty),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Selector {
    pub owner: Ty,
    pub name: Symbol,
    pub params: Vec<Ty>,
    pub method_args: Vec<Ty>,
    pub receiver: ReceiverKind,
    pub ret: Ty,
}

#[derive(Clone, Debug)]
pub(crate) struct Resolved {
    pub method: MethodSig,
    pub owner_args: Vec<Ty>,
    pub method_args: Vec<Ty>,
}

impl Cx<'_, '_> {
    pub(crate) fn root_body(&self, mut body: BodyId) -> BodyId {
        while let BodySource::Closure { parent, .. } = self.mir.body(body).source {
            body = parent;
        }
        body
    }

    pub(crate) fn body_generics(&self, body: BodyId) -> Vec<Symbol> {
        let root = self.mir.body(self.root_body(body));
        let sig = self.tcx.sig(root.owner);
        let method = method_of(self.tcx, root.owner, root.member);
        let owner_generics = match (root.source, method) {
            (BodySource::FieldInit, _) => self.field_is_instance(root.owner, root.name),
            (_, Some(method)) => method.receiver.is_some() || self.is_ctor(method),
            (_, None) => true,
        };
        let mut generics: Vec<Symbol> = Vec::new();
        if owner_generics {
            generics.extend(sig.generics.iter().map(|param| param.name));
        }
        if let Some(method) = method {
            generics.extend(method.generics.iter().map(|param| param.name));
        }
        generics
    }

    fn field_is_instance(&self, owner: DefId, name: Symbol) -> bool {
        self.tcx
            .sig(owner)
            .fields
            .get(&name)
            .is_some_and(|field| matches!(field.storage, krelpin_ast::ast::FieldStorage::Instance))
    }

    pub(crate) fn is_ctor(&self, method: &MethodSig) -> bool {
        self.tcx
            .sig(method.owner)
            .ctors
            .iter()
            .any(|ctor| ctor.member == method.member && ctor.name == method.name)
    }

    pub(crate) fn method_generics(&self, method: &MethodSig) -> Vec<Symbol> {
        let mut generics = Vec::new();
        if method.receiver.is_some() || self.is_ctor(method) {
            generics.extend(self.tcx.sig(method.owner).generics.iter().map(|p| p.name));
        }
        generics.extend(method.generics.iter().map(|param| param.name));
        generics
    }

    pub(crate) fn subst_of(&self, names: &[Symbol], args: &[Ty]) -> Subst {
        names.iter().copied().zip(args.iter().cloned()).collect()
    }

    pub(crate) fn instance_for(
        &mut self,
        method: &MethodSig,
        subst: &Subst,
        self_ty: &Ty,
    ) -> Instance {
        let names = self.method_generics(method);
        let args: Vec<Ty> = names
            .iter()
            .map(|name| subst.get(name).cloned().unwrap_or(Ty::Void))
            .collect();
        match method
            .member
            .and_then(|member| self.body_of.get(&(method.owner, member)))
        {
            Some(&body) => Instance::Body { body, args },
            None => Instance::Builtin {
                owner: method.owner,
                name: method.name.name,
                member: method.member,
                args,
                self_ty: self_ty.clone(),
            },
        }
    }

    pub(crate) fn adt_of(&self, ty: &Ty) -> Option<(DefId, Vec<Ty>)> {
        match ty {
            Ty::Adt { def, args, .. } => Some((*def, args.clone())),
            Ty::Primitive(primitive) => self
                .tcx
                .lang_items
                .primitive_impl(*primitive)
                .map(|def| (def, Vec::new())),
            _ => None,
        }
    }

    pub(crate) fn is_subtype(&self, ty: &Ty, owner: &Ty) -> bool {
        let Some((owner_def, owner_args)) = self.adt_of(owner) else {
            return false;
        };
        if self.tcx.lang_items.is(owner_def, LangItem::Object) {
            return true;
        }
        let Some((def, args)) = self.adt_of(ty) else {
            return false;
        };
        self.tcx.upcast(def, &args, owner_def).as_deref() == Some(owner_args.as_slice())
    }

    pub(crate) fn find_impl(&self, ty: &Ty, selector: &Selector) -> Option<Resolved> {
        let (def, args) = self.adt_of(ty)?;
        let mut chain = vec![(def, args.clone())];
        let mut current = (def, args);
        while let Some(parent) = self.tcx.sig(current.0).superclass.clone() {
            let parent = parent.subst(&self.tcx.subst_for(current.0, &current.1));
            let Ty::Adt { def, args, .. } = parent else {
                break;
            };
            chain.push((def, args.clone()));
            current = (def, args);
        }
        if self.tcx.sig(def).kind != AdtKind::Interface
            && let Some(object) = self.tcx.lang_items.get(LangItem::Object)
            && chain.iter().all(|(seen, _)| *seen != object)
        {
            chain.push((object, Vec::new()));
        }
        for (class, class_args) in &chain {
            if let Some(found) = self.match_method(*class, class_args, selector, false) {
                return Some(found);
            }
        }
        let (def, args) = self.adt_of(ty)?;
        let self_subst = self.tcx.subst_for(def, &args);
        for (parent, parent_subst) in self.tcx.supertypes(def) {
            if self.tcx.sig(parent).kind != AdtKind::Interface {
                continue;
            }
            let parent_args: Vec<Ty> = self
                .tcx
                .sig(parent)
                .generics
                .iter()
                .map(|param| {
                    parent_subst
                        .get(&param.name)
                        .map_or(Ty::Unknown, |ty| ty.subst(&self_subst))
                })
                .collect();
            if let Some(found) = self.match_method(parent, &parent_args, selector, true) {
                return Some(found);
            }
        }
        None
    }

    fn match_method(
        &self,
        def: DefId,
        args: &[Ty],
        selector: &Selector,
        defaults: bool,
    ) -> Option<Resolved> {
        let sig = self.tcx.sig(def);
        let methods = sig.methods.get(&selector.name)?;
        let owner_subst = self.tcx.subst_for(def, args);
        methods.iter().find_map(|method| {
            if method.receiver.is_none() || method.is_abstract {
                return None;
            }
            if defaults && !self.has_code(method) {
                return None;
            }
            if method.generics.len() != selector.method_args.len()
                || method.params.len() != selector.params.len()
            {
                return None;
            }
            let mut subst = owner_subst.clone();
            for (param, arg) in method.generics.iter().zip(&selector.method_args) {
                subst.insert(param.name, arg.clone());
            }
            let params: Vec<Ty> = method.params.iter().map(|ty| ty.subst(&subst)).collect();
            (params == selector.params).then(|| Resolved {
                method: method.clone(),
                owner_args: args.to_vec(),
                method_args: selector.method_args.clone(),
            })
        })
    }

    fn has_code(&self, method: &MethodSig) -> bool {
        !method.is_abstract
    }

    pub(crate) fn resolved_instance(&mut self, resolved: &Resolved, self_ty: &Ty) -> Instance {
        let mut subst = self
            .tcx
            .subst_for(resolved.method.owner, &resolved.owner_args);
        for (param, arg) in resolved.method.generics.iter().zip(&resolved.method_args) {
            subst.insert(param.name, arg.clone());
        }
        self.instance_for(&resolved.method.clone(), &subst, self_ty)
    }

    pub(crate) fn receiver_kind(&self, method: &MethodSig) -> Option<ReceiverKind> {
        if self.is_ctor(method) {
            return Some(ReceiverKind::RefMut);
        }
        method.receiver
    }
}

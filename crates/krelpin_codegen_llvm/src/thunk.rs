use krelpin_ast::ast::ReceiverKind;
use krelpin_typeck::{LangItem, Subst, Ty};

use crate::cx::Cx;
use crate::func::{FnCx, Val};
use crate::instance::{Instance, Selector, Thunk};
use crate::rtti::ClosureKind;

impl Cx<'_, '_> {
    pub(crate) fn emit_instance(&mut self, id: usize, instance: &Instance) -> String {
        match instance {
            Instance::Body { body, args } => self.emit_body(id, *body, args),
            Instance::Builtin {
                owner,
                name,
                member,
                args,
                self_ty,
            } => self.emit_builtin(id, *owner, *name, *member, args, self_ty),
            Instance::Thunk(thunk) => self.emit_thunk(id, thunk),
        }
    }

    fn selector_signature(&mut self, selector: &Selector) -> (Vec<String>, String) {
        let mut params = vec!["ptr %recv".to_owned()];
        for (index, param) in selector.params.iter().enumerate() {
            let ll = self.llty(param);
            params.push(format!("{ll} %p{index}"));
        }
        (params, self.llty(&selector.ret))
    }

    fn emit_thunk(&mut self, id: usize, thunk: &Thunk) -> String {
        let name = self.fn_name(id);
        match thunk {
            Thunk::Boxed {
                value,
                selector,
                target,
            } => {
                let selector = self.selectors[*selector].clone();
                let (params, ret) = self.selector_signature(&selector);
                let target_params = self.instance_params(target);
                let target_ret = self.instance_ret(target);
                let resolved = self.receiver_of(target);
                let target_id = self.instance(target.as_ref().clone());
                let mut fx = FnCx::new(self, Subst::new(), selector.ret.clone());
                fx.start("entry");
                let object = match selector.receiver {
                    ReceiverKind::Owned => "%recv".to_owned(),
                    _ => fx.assign("load ptr, ptr %recv"),
                };
                let takes_object = target_params
                    .first()
                    .is_some_and(|ty| !fx.cx.is_value_type(ty.strip_refs()));
                let boxed = fx.cx.is_value_type(value) && !takes_object;
                let mut cleanup = None;
                let receiver = if boxed {
                    let box_ty = fx.cx.box_type(value);
                    let payload = fx.assign(format!(
                        "getelementptr inbounds {box_ty}, ptr {object}, i32 0, i32 1"
                    ));
                    match resolved {
                        Some(ReceiverKind::Owned) => {
                            let loaded = fx.load(value, &payload);
                            if selector.receiver == ReceiverKind::Owned {
                                fx.runtime("void", "kr_free", &format!("ptr {object}"));
                                loaded
                            } else {
                                fx.copy_value(&loaded, value)
                            }
                        }
                        _ => {
                            if selector.receiver == ReceiverKind::Owned {
                                cleanup = Some(object.clone());
                            }
                            payload
                        }
                    }
                } else {
                    match (selector.receiver, resolved) {
                        (ReceiverKind::Owned, Some(ReceiverKind::Ref | ReceiverKind::RefMut)) => {
                            let slot = fx.alloca_ll("ptr");
                            fx.line(format!("store ptr {object}, ptr {slot}"));
                            cleanup = Some(object.clone());
                            slot
                        }
                        (_, Some(ReceiverKind::Owned)) => object.clone(),
                        _ => "%recv".to_owned(),
                    }
                };
                let receiver_ll = target_params
                    .first()
                    .map_or("ptr".to_owned(), |ty| fx.ll(ty));
                let mut args = vec![(receiver_ll, receiver)];
                for (index, param) in selector.params.iter().enumerate() {
                    let target_param = target_params
                        .get(index + 1)
                        .cloned()
                        .unwrap_or(param.clone());
                    let converted = fx.convert(&format!("%p{index}"), param, &target_param);
                    let ll = fx.ll(&target_param);
                    args.push((ll, converted));
                }
                let result = fx.call_instance(target_id, &args, &target_ret);
                if let Some(object) = cleanup {
                    fx.drop_value(&object, &selector.owner);
                }
                fx.finish_return(&result, &target_ret, &selector.ret);
                fx.into_function(&name, &params, &ret)
            }
            Thunk::Lambda { closure, selector } => {
                let selector = self.selectors[*selector].clone();
                let (params, ret) = self.selector_signature(&selector);
                let ClosureKind::Lambda { body, args } = self.closures[*closure].kind.clone()
                else {
                    panic!("closure {closure} is a lambda");
                };
                let target = Instance::Body { body, args };
                let target_params = self.instance_params(&target);
                let target_ret = self.instance_ret(&target);
                let target_id = self.instance(target);
                let mut fx = FnCx::new(self, Subst::new(), selector.ret.clone());
                fx.start("entry");
                let mut call_args = vec![("ptr".to_owned(), "%recv".to_owned())];
                for (index, param) in selector.params.iter().enumerate() {
                    let target_param = target_params
                        .get(index + 1)
                        .cloned()
                        .unwrap_or(param.clone());
                    let converted = fx.convert(&format!("%p{index}"), param, &target_param);
                    let ll = fx.ll(&target_param);
                    call_args.push((ll, converted));
                }
                let result = fx.call_instance(target_id, &call_args, &target_ret);
                fx.finish_return(&result, &target_ret, &selector.ret);
                fx.into_function(&name, &params, &ret)
            }
            Thunk::MethodRef { closure, selector } => {
                let selector = self.selectors[*selector].clone();
                self.emit_method_ref(&name, *closure, &selector)
            }
            Thunk::ThreadEntry { runnable } => {
                let runnable = runnable.clone();
                let message = self.message_instance();
                let mut fx = FnCx::new(self, Subst::new(), Ty::Void);
                fx.start("entry");
                let def = fx.cx.lang(LangItem::Thread);
                let _ = def;
                let run = Selector {
                    owner: runnable.clone(),
                    name: krelpin_span::Symbol::intern("run"),
                    params: Vec::new(),
                    method_args: Vec::new(),
                    receiver: ReceiverKind::RefMut,
                    ret: Ty::Void,
                };
                let receiver_ty = Ty::Ref {
                    mutable: true,
                    ty: Box::new(runnable),
                };
                fx.dispatch(&run, &receiver_ty, "%recv", &[]);
                let pending = fx.assign("load ptr, ptr @kr_pending");
                let failed = fx.assign(format!("icmp ne ptr {pending}, null"));
                fx.line(format!("br i1 {failed}, label %report, label %done"));
                fx.start("report");
                fx.line("store ptr null, ptr @kr_pending");
                let slot = fx.alloca_ll("ptr");
                fx.line(format!("store ptr {pending}, ptr {slot}"));
                let text = fx.call_instance(
                    message,
                    &[("ptr".to_owned(), slot.clone())],
                    &Ty::Nullable(Box::new(fx.cx.tcx.string_ty())),
                );
                let descriptor = fx.assign(format!("load ptr, ptr {pending}"));
                fx.runtime(
                    "void",
                    "kr_report_thread",
                    &format!("ptr {descriptor}, ptr {text}"),
                );
                let string = fx.cx.tcx.string_ty();
                fx.drop_value(&text, &string);
                let throwable = fx.cx.lang_ty(LangItem::Throwable);
                fx.drop_value(&pending, &throwable);
                fx.line("br label %done");
                fx.start("done");
                fx.line("ret void");
                fx.into_function(&name, &["ptr %recv".to_owned()], "void")
            }
            Thunk::TableHash(key) => {
                let key = key.clone();
                let mut fx = FnCx::new(self, Subst::new(), Ty::INT);
                fx.start("entry");
                let hash = fx.call_hash("%recv", &key);
                fx.line(format!("ret i32 {hash}"));
                fx.into_function(&name, &["ptr %recv".to_owned()], "i32")
            }
            Thunk::TableEq(key) => {
                let key = key.clone();
                let mut fx = FnCx::new(self, Subst::new(), Ty::BOOLEAN);
                fx.start("entry");
                let equal = fx.call_equals("%recv", "%p0", &key);
                fx.line(format!("ret i1 {equal}"));
                fx.into_function(
                    &name,
                    &["ptr %recv".to_owned(), "ptr %p0".to_owned()],
                    "zeroext i1",
                )
            }
            Thunk::TableDrop(ty) => {
                let ty = ty.clone();
                let mut fx = FnCx::new(self, Subst::new(), Ty::Void);
                fx.start("entry");
                fx.drop_at(&ty, "%recv");
                fx.line("ret void");
                fx.into_function(&name, &["ptr %recv".to_owned()], "void")
            }
            Thunk::DropObject(ty) => {
                let ty = ty.clone();
                let mut fx = FnCx::new(self, Subst::new(), Ty::Void);
                fx.start("entry");
                fx.emit_drop_object_at("%recv", &ty);
                fx.line("ret void");
                fx.into_function(&name, &["ptr %recv".to_owned()], "void")
            }
        }
    }

    fn receiver_of(&self, instance: &Instance) -> Option<ReceiverKind> {
        match instance {
            Instance::Body { body, .. } => {
                let mir = self.mir.body(*body);
                if mir.arg_count == 0 {
                    return None;
                }
                match &mir.locals[1].ty {
                    Ty::Ref { mutable, .. } => Some(if *mutable {
                        ReceiverKind::RefMut
                    } else {
                        ReceiverKind::Ref
                    }),
                    _ => Some(ReceiverKind::Owned),
                }
            }
            Instance::Builtin {
                owner,
                name,
                member,
                ..
            } => {
                let method = self.builtin_method(*owner, *name, *member);
                self.receiver_kind(&method)
            }
            _ => None,
        }
    }

    pub(crate) fn message_instance(&mut self) -> usize {
        let throwable = self.lang(LangItem::Throwable);
        let method = self
            .tcx
            .sig(throwable)
            .methods
            .get(&krelpin_span::Symbol::intern("getMessage"))
            .and_then(|methods| methods.first())
            .cloned()
            .expect("`Throwable` declares `getMessage`");
        let ty = self.tcx.self_ty(throwable);
        let instance = self.instance_for(&method, &Subst::new(), &ty);
        self.instance(instance)
    }

    fn emit_method_ref(&mut self, name: &str, closure: usize, selector: &Selector) -> String {
        let (params, ret) = self.selector_signature(selector);
        let ClosureKind::MethodRef {
            callee,
            subst,
            bound,
            constructs,
        } = self.closures[closure].kind.clone()
        else {
            panic!("closure {closure} is a method reference");
        };
        let closure_ll = self.closure_type(closure);
        let mut fx = FnCx::new(self, subst.clone(), selector.ret.clone());
        fx.start("entry");
        let incoming: Vec<Val> = selector
            .params
            .iter()
            .enumerate()
            .map(|(index, ty)| Val {
                ty: ty.clone(),
                v: format!("%p{index}"),
            })
            .collect();
        if constructs {
            let object_ty = match &callee {
                Some(callee) => fx.cx.tcx.self_ty(callee.method.owner).subst(
                    &callee
                        .subst
                        .iter()
                        .map(|(name, ty)| (*name, ty.subst(&subst)))
                        .collect(),
                ),
                None => selector.ret.clone(),
            };
            let object = fx.new_object(&object_ty, None);
            if let Some(callee) = &callee {
                let slot = fx.alloca(&object_ty);
                fx.store(&object_ty, &object, &slot);
                let full: Subst = callee
                    .subst
                    .iter()
                    .map(|(name, ty)| (*name, ty.subst(&subst)))
                    .collect();
                let instance = fx.cx.instance_for(&callee.method, &full, &object_ty);
                let target_params = fx.cx.instance_params(&instance);
                let mut args = vec![("ptr".to_owned(), slot.clone())];
                for (index, value) in incoming.iter().enumerate() {
                    let param = target_params
                        .get(index + 1)
                        .cloned()
                        .unwrap_or(value.ty.clone());
                    let converted = fx.convert(&value.v, &value.ty, &param);
                    let ll = fx.ll(&param);
                    args.push((ll, converted));
                }
                let id = fx.cx.instance(instance);
                fx.call_instance(id, &args, &Ty::Void);
                let built = fx.load(&object_ty, &slot);
                fx.line(format!("store ptr null, ptr {slot}"));
                fx.finish_return(&built, &object_ty, &selector.ret);
            } else {
                fx.finish_return(&object, &object_ty, &selector.ret);
            }
            return fx.into_function(name, &params, &ret);
        }
        let callee = callee.expect("a method reference names a method");
        let full: Subst = callee
            .subst
            .iter()
            .map(|(name, ty)| (*name, ty.subst(&subst)))
            .collect();
        let method = callee.method.clone();
        let receiver_kind = fx.cx.receiver_kind(&method);
        let Some(kind) = receiver_kind else {
            let instance = fx
                .cx
                .instance_for(&method, &full, &fx.cx.tcx.self_ty(method.owner));
            let target_params = fx.cx.instance_params(&instance);
            let target_ret = fx.cx.instance_ret(&instance);
            let mut args = Vec::new();
            for (index, value) in incoming.iter().enumerate() {
                let param = target_params
                    .get(index)
                    .cloned()
                    .unwrap_or(value.ty.clone());
                let converted = fx.convert(&value.v, &value.ty, &param);
                let ll = fx.ll(&param);
                args.push((ll, converted));
            }
            let id = fx.cx.instance(instance);
            let result = fx.call_instance(id, &args, &target_ret);
            fx.finish_return(&result, &target_ret, &selector.ret);
            return fx.into_function(name, &params, &ret);
        };
        let (receiver_value, receiver_ty, rest, cleanup) = match &bound {
            Some(bound_ty) => {
                let object = match selector.receiver {
                    ReceiverKind::Owned => "%recv".to_owned(),
                    _ => fx.assign("load ptr, ptr %recv"),
                };
                let address = fx.assign(format!(
                    "getelementptr inbounds {closure_ll}, ptr {object}, i32 0, i32 1"
                ));
                let (value, ty) = fx.receiver_from_slot(&address, bound_ty, kind);
                (value, ty, incoming.clone(), None)
            }
            None => {
                let first = incoming[0].clone();
                let (value, ty, cleanup) = fx.receiver_from_value(&first, kind);
                (value, ty, incoming[1..].to_vec(), cleanup)
            }
        };
        let target_selector = fx.cx.selector_for(&callee, &subst);
        let result = fx.dispatch(&target_selector, &receiver_ty, &receiver_value, &rest);
        if let Some((slot, ty)) = cleanup {
            fx.drop_at(&ty, &slot);
        }
        let result_ty = target_selector.ret.clone();
        fx.finish_return(&result, &result_ty, &selector.ret);
        fx.into_function(name, &params, &ret)
    }
}

impl FnCx<'_, '_, '_> {
    pub(crate) fn finish_return(&mut self, value: &str, from: &Ty, to: &Ty) {
        let ll = self.ll(to);
        if ll == "void" {
            if *from != Ty::Void {
                self.drop_value(value, from);
            }
            self.line("ret void");
        } else {
            let converted = self.convert(value, from, to);
            self.line(format!("ret {ll} {converted}"));
        }
        self.finish_fail();
    }

    pub(crate) fn into_function(self, name: &str, params: &[String], ret: &str) -> String {
        format!(
            "define internal {ret} {name}({}) {{\nallocas:\n{}  br label %entry\n{}}}\n",
            params.join(", "),
            self.allocas,
            self.out
        )
    }

    fn receiver_from_slot(&mut self, address: &str, ty: &Ty, kind: ReceiverKind) -> (String, Ty) {
        match (ty, kind) {
            (Ty::Ref { .. }, ReceiverKind::Ref | ReceiverKind::RefMut) => {
                let pointer = self.assign(format!("load ptr, ptr {address}"));
                (pointer, ty.clone())
            }
            (_, ReceiverKind::Ref | ReceiverKind::RefMut) => (
                address.to_owned(),
                Ty::Ref {
                    mutable: kind == ReceiverKind::RefMut,
                    ty: Box::new(ty.clone()),
                },
            ),
            (Ty::Ref { ty: inner, .. }, ReceiverKind::Owned) => {
                let inner = inner.as_ref().clone();
                let pointer = self.assign(format!("load ptr, ptr {address}"));
                let value = self.load(&inner, &pointer);
                (self.copy_value(&value, &inner), inner)
            }
            (_, ReceiverKind::Owned) => {
                let value = self.load(ty, address);
                (self.copy_value(&value, ty), ty.clone())
            }
        }
    }

    fn receiver_from_value(
        &mut self,
        value: &Val,
        kind: ReceiverKind,
    ) -> (String, Ty, Option<(String, Ty)>) {
        match (&value.ty, kind) {
            (Ty::Ref { .. }, ReceiverKind::Ref | ReceiverKind::RefMut) => {
                (value.v.clone(), value.ty.clone(), None)
            }
            (_, ReceiverKind::Ref | ReceiverKind::RefMut) => {
                let slot = self.alloca(&value.ty);
                self.store(&value.ty, &value.v, &slot);
                (
                    slot.clone(),
                    Ty::Ref {
                        mutable: kind == ReceiverKind::RefMut,
                        ty: Box::new(value.ty.clone()),
                    },
                    Some((slot, value.ty.clone())),
                )
            }
            (Ty::Ref { ty: inner, .. }, ReceiverKind::Owned) => {
                let inner = inner.as_ref().clone();
                let loaded = self.load(&inner, &value.v);
                (self.copy_value(&loaded, &inner), inner, None)
            }
            (_, ReceiverKind::Owned) => (value.v.clone(), value.ty.clone(), None),
        }
    }

    pub(crate) fn emit_drop_object_at(&mut self, object: &str, ty: &Ty) {
        self.emit_drop_object(object, ty);
    }
}

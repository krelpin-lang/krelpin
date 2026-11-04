use krelpin_ast::ast::{PrimitiveTy, ReceiverKind};
use krelpin_errors::Diagnostic;
use krelpin_span::Symbol;
use krelpin_span::def_id::DefId;
use krelpin_typeck::{AdtKind, LangItem, Ty};

use crate::cx::Cx;
use crate::func::{FnCx, Val, array_ll, float_const};
use crate::instance::{Instance, Thunk};
use crate::layout::{PtrKind, Shape};
use crate::rtti::RtKey;

struct Builtin {
    params: Vec<Ty>,
    self_ty: Ty,
    receiver: Option<ReceiverKind>,
}

impl Builtin {
    fn arg(&self, index: usize) -> String {
        let offset = usize::from(self.receiver.is_some());
        format!("%a{}", index + offset)
    }

    fn arg_ty(&self, index: usize) -> Ty {
        let offset = usize::from(self.receiver.is_some());
        self.params[index + offset].clone()
    }
}

impl Cx<'_, '_> {
    pub(crate) fn emit_builtin(
        &mut self,
        id: usize,
        owner: DefId,
        name: Symbol,
        member: Option<u32>,
        args: &[Ty],
        self_ty: &Ty,
    ) -> String {
        let instance = Instance::Builtin {
            owner,
            name,
            member,
            args: args.to_vec(),
            self_ty: self_ty.clone(),
        };
        let method = self.builtin_method(owner, name, member);
        let params = self.instance_params(&instance);
        let ret = self.instance_ret(&instance);
        let builtin = Builtin {
            receiver: self.receiver_kind(&method),
            params: params.clone(),
            self_ty: self_ty.clone(),
        };
        let fn_name = self.fn_name(id);
        let rendered: Vec<String> = params
            .iter()
            .enumerate()
            .map(|(index, ty)| format!("{} %a{index}", self.llty(ty)))
            .collect();
        let llret = self.llty(&ret);
        let owner_name = self.tcx.sig(owner).name.name.to_string();
        let mut fx = FnCx::new(self, Default::default(), ret.clone());
        fx.start("entry");
        let handled = fx.builtin_body(owner, name.as_str(), &builtin);
        if !handled {
            fx.cx.error(Diagnostic::error(format!(
                "the code generator does not support the intrinsic `{owner_name}.{name}` yet"
            )));
            fx.ret_zero(&ret);
        }
        fx.finish_fail();
        format!(
            "define internal {llret} {fn_name}({}) {{\nallocas:\n{}  br label %entry\n{}}}\n",
            rendered.join(", "),
            fx.allocas,
            fx.out
        )
    }
}

impl FnCx<'_, '_, '_> {
    fn ret(&mut self, value: &str, ty: &Ty) {
        let ret = self.ret_ty.clone();
        let ll = self.ll(&ret);
        if ll == "void" {
            self.line("ret void");
            return;
        }
        let converted = self.convert(value, ty, &ret);
        self.line(format!("ret {ll} {converted}"));
    }

    fn this_value(&mut self, builtin: &Builtin) -> String {
        match builtin.receiver {
            Some(ReceiverKind::Owned) | None => "%a0".to_owned(),
            Some(_) => {
                let ty = builtin.self_ty.clone();
                self.load(&ty, "%a0")
            }
        }
    }

    fn builtin_body(&mut self, owner: DefId, name: &str, builtin: &Builtin) -> bool {
        let items = &self.cx.tcx.lang_items;
        let kind = self.cx.tcx.sig(owner).kind;
        if items.is(owner, LangItem::Object) && name == "toString" {
            let text = self.default_to_string(builtin);
            let string = self.cx.tcx.string_ty();
            self.ret(&text, &string);
            return true;
        }
        if items.is(owner, LangItem::Enum)
            || (kind == AdtKind::Enum && builtin.receiver.is_some())
            || (kind == AdtKind::Enum && name == "values")
            || (kind == AdtKind::Record && matches!(name, "equals" | "hashCode"))
        {
            return self.adt_builtin(name, builtin);
        }
        if items.is(owner, LangItem::String) {
            return self.string_builtin(name, builtin);
        }
        if items.is(owner, LangItem::Console) {
            return self.console_builtin(name, builtin);
        }
        if items.is(owner, LangItem::Math) {
            return self.math_builtin(name, builtin);
        }
        if let Some(primitive) = items.primitive_of(owner) {
            return self.primitive_builtin(primitive, name, builtin);
        }
        if items.is(owner, LangItem::Thread) {
            return self.thread_builtin(name, builtin);
        }
        if items.is(owner, LangItem::AtomicInt)
            || items.is(owner, LangItem::AtomicLong)
            || items.is(owner, LangItem::AtomicBool)
        {
            return self.atomic_builtin(owner, name, builtin);
        }
        if items.is(owner, LangItem::Mutex) || items.is(owner, LangItem::MutexGuard) {
            return self.mutex_builtin(owner, name, builtin);
        }
        if items.is(owner, LangItem::ArrayList) {
            return self.list_builtin(name, builtin);
        }
        if items.is(owner, LangItem::HashMap) || items.is(owner, LangItem::HashSet) {
            return self.table_builtin(owner, name, builtin);
        }
        false
    }

    fn default_to_string(&mut self, builtin: &Builtin) -> String {
        let ty = builtin.self_ty.clone();
        match self.cx.shape(&ty) {
            Shape::Prim(primitive) => {
                let value = self.load(&ty, "%a0");
                self.prim_to_string(&value, primitive)
            }
            Shape::Ptr(PtrKind::String) => {
                let value = self.load(&ty, "%a0");
                let string = self.cx.tcx.string_ty();
                self.copy_value(&value, &string)
            }
            Shape::Record(def, args) => {
                let name = self.cx.tcx.sig(def).name.name.to_string();
                let fields = self.cx.record_fields(def, &args);
                let record_ll = self.ll(&ty);
                let mut parts = Vec::new();
                for (index, (field, field_ty)) in fields.iter().enumerate() {
                    let address = self.assign(format!(
                        "getelementptr inbounds {record_ll}, ptr %a0, i32 0, i32 {index}"
                    ));
                    let address = self.indirect_slot(&ty, field_ty, address);
                    parts.push((field.to_string(), self.value_to_string(&address, field_ty)));
                }
                self.join_parts(&format!("{name}["), &parts, "]", true)
            }
            Shape::Enum(def, args) => {
                let enum_ll = self.ll(&ty);
                let tag_addr = self.assign(format!(
                    "getelementptr inbounds {enum_ll}, ptr %a0, i32 0, i32 0"
                ));
                let tag = self.assign(format!("load i32, ptr {tag_addr}"));
                let variants = self.cx.tcx.sig(def).variants.clone();
                let result = self.alloca_ll("ptr");
                let done = self.label("named");
                let mut cases = Vec::new();
                let mut blocks = Vec::new();
                for (index, _) in variants.iter().enumerate() {
                    let label = self.label("variant");
                    cases.push(format!("i32 {index}, label %{label}"));
                    blocks.push(label);
                }
                self.line(format!(
                    "switch i32 {tag}, label %{done} [ {} ]",
                    cases.join(" ")
                ));
                for (index, variant) in variants.iter().enumerate() {
                    self.start(&blocks[index]);
                    let fields = self.cx.variant_fields(def, &args, index);
                    let text = if fields.is_empty() {
                        self.cx.string_literal(variant.name.name.as_str())
                    } else {
                        let variant_ll = self.cx.variant_type(def, &args, index);
                        let mut parts = Vec::new();
                        for (position, (field, field_ty)) in fields.iter().enumerate() {
                            let address = self.assign(format!(
                                "getelementptr inbounds {variant_ll}, ptr %a0, i32 0, i32 {}",
                                position + 1
                            ));
                            let address = self.indirect_slot(&ty, field_ty, address);
                            parts.push((
                                field.to_string(),
                                self.value_to_string(&address, field_ty),
                            ));
                        }
                        self.join_parts(&format!("{}(", variant.name.name), &parts, ")", false)
                    };
                    self.line(format!("store ptr {text}, ptr {result}"));
                    self.line(format!("br label %{done}"));
                }
                self.start(&done);
                self.assign(format!("load ptr, ptr {result}"))
            }
            _ => {
                let object = self.this_value(builtin);
                self.runtime("ptr", "kr_string_describe", &format!("ptr {object}"))
            }
        }
    }

    fn join_parts(
        &mut self,
        open: &str,
        parts: &[(String, String)],
        close: &str,
        named: bool,
    ) -> String {
        let string = self.cx.tcx.string_ty();
        let mut current = self.cx.string_literal(open);
        for (index, (name, text)) in parts.iter().enumerate() {
            let prefix = match (index, named) {
                (0, true) => format!("{name}="),
                (0, false) => String::new(),
                (_, true) => format!(", {name}="),
                (_, false) => ", ".to_owned(),
            };
            if !prefix.is_empty() {
                let literal = self.cx.string_literal(&prefix);
                let joined = self.runtime(
                    "ptr",
                    "kr_string_concat",
                    &format!("ptr {current}, ptr {literal}"),
                );
                self.drop_value(&current, &string);
                current = joined;
            }
            let joined = self.runtime(
                "ptr",
                "kr_string_concat",
                &format!("ptr {current}, ptr {text}"),
            );
            self.drop_value(&current, &string);
            self.drop_value(text, &string);
            current = joined;
        }
        let literal = self.cx.string_literal(close);
        let joined = self.runtime(
            "ptr",
            "kr_string_concat",
            &format!("ptr {current}, ptr {literal}"),
        );
        self.drop_value(&current, &string);
        joined
    }

    fn adt_builtin(&mut self, name: &str, builtin: &Builtin) -> bool {
        let ty = builtin.self_ty.clone();
        match (self.cx.shape(&ty), name) {
            (Shape::Enum(..), "ordinal") => {
                let enum_ll = self.ll(&ty);
                let tag = self.assign(format!(
                    "getelementptr inbounds {enum_ll}, ptr %a0, i32 0, i32 0"
                ));
                let value = self.assign(format!("load i32, ptr {tag}"));
                self.ret(&value, &Ty::INT);
                true
            }
            (Shape::Enum(def, _), "name") => {
                let enum_ll = self.ll(&ty);
                let tag = self.assign(format!(
                    "getelementptr inbounds {enum_ll}, ptr %a0, i32 0, i32 0"
                ));
                let tag = self.assign(format!("load i32, ptr {tag}"));
                let variants = self.cx.tcx.sig(def).variants.clone();
                let result = self.alloca_ll("ptr");
                let done = self.label("named");
                let mut cases = Vec::new();
                for (index, variant) in variants.iter().enumerate() {
                    let label = self.label("variant");
                    cases.push((index, label, variant.name.name));
                }
                let rendered: Vec<String> = cases
                    .iter()
                    .map(|(index, label, _)| format!("i32 {index}, label %{label}"))
                    .collect();
                self.line(format!(
                    "switch i32 {tag}, label %{done} [ {} ]",
                    rendered.join(" ")
                ));
                for (_, label, name) in cases {
                    self.start(&label);
                    let literal = self.cx.string_literal(name.as_str());
                    self.line(format!("store ptr {literal}, ptr {result}"));
                    self.line(format!("br label %{done}"));
                }
                self.start(&done);
                let value = self.assign(format!("load ptr, ptr {result}"));
                let string = self.cx.tcx.string_ty();
                self.ret(&value, &string);
                true
            }
            (Shape::Enum(def, args), "values") => {
                let variants = self.cx.tcx.sig(def).variants.len();
                let ret = self.ret_ty.clone();
                let array = self.new_array(&ret, &variants.to_string());
                let elem_ll = self.cx.mem_llty(&ty);
                for index in 0..variants {
                    let variant_ll = self.cx.variant_type(def, &args, index);
                    let address = self.assign(format!(
                        "getelementptr inbounds {}, ptr {array}, i32 0, i32 2, i64 {index}",
                        array_ll(&elem_ll)
                    ));
                    let tag = self.assign(format!(
                        "getelementptr inbounds {variant_ll}, ptr {address}, i32 0, i32 0"
                    ));
                    self.line(format!("store i32 {index}, ptr {tag}"));
                }
                self.ret(&array, &ret);
                true
            }
            (Shape::Enum(def, args), "equals") => {
                let enum_ll = self.ll(&ty);
                let a = self.assign(format!(
                    "getelementptr inbounds {enum_ll}, ptr %a0, i32 0, i32 0"
                ));
                let b = self.assign(format!(
                    "getelementptr inbounds {enum_ll}, ptr %a1, i32 0, i32 0"
                ));
                let a = self.assign(format!("load i32, ptr {a}"));
                let b = self.assign(format!("load i32, ptr {b}"));
                let same = self.assign(format!("icmp eq i32 {a}, {b}"));
                let _ = args;
                let _ = def;
                self.ret(&same, &Ty::BOOLEAN);
                true
            }
            (Shape::Enum(..), "hashCode") => {
                let enum_ll = self.ll(&ty);
                let a = self.assign(format!(
                    "getelementptr inbounds {enum_ll}, ptr %a0, i32 0, i32 0"
                ));
                let a = self.assign(format!("load i32, ptr {a}"));
                self.ret(&a, &Ty::INT);
                true
            }
            (Shape::Record(def, args), "equals") => {
                let record_ll = self.ll(&ty);
                let fields = self.cx.record_fields(def, &args);
                let mut result = "true".to_owned();
                for (index, (_, field_ty)) in fields.iter().enumerate() {
                    let a = self.assign(format!(
                        "getelementptr inbounds {record_ll}, ptr %a0, i32 0, i32 {index}"
                    ));
                    let b = self.assign(format!(
                        "getelementptr inbounds {record_ll}, ptr %a1, i32 0, i32 {index}"
                    ));
                    let a = self.indirect_slot(&ty, field_ty, a);
                    let b = self.indirect_slot(&ty, field_ty, b);
                    let equal = self.field_equals(&a, &b, field_ty);
                    result = self.assign(format!("and i1 {result}, {equal}"));
                }
                self.ret(&result, &Ty::BOOLEAN);
                true
            }
            (Shape::Record(def, args), "hashCode") => {
                let record_ll = self.ll(&ty);
                let fields = self.cx.record_fields(def, &args);
                let mut hash = "0".to_owned();
                for (index, (_, field_ty)) in fields.iter().enumerate() {
                    let a = self.assign(format!(
                        "getelementptr inbounds {record_ll}, ptr %a0, i32 0, i32 {index}"
                    ));
                    let a = self.indirect_slot(&ty, field_ty, a);
                    let field_hash = self.field_hash(&a, field_ty);
                    let scaled = self.assign(format!("mul i32 {hash}, 31"));
                    hash = self.assign(format!("add i32 {scaled}, {field_hash}"));
                }
                self.ret(&hash, &Ty::INT);
                true
            }
            _ => false,
        }
    }

    fn field_equals(&mut self, a: &str, b: &str, ty: &Ty) -> String {
        match self.cx.shape(ty) {
            Shape::Prim(PrimitiveTy::Float | PrimitiveTy::Double) => {
                let ll = self.ll(ty);
                let a = self.assign(format!("load {ll}, ptr {a}"));
                let b = self.assign(format!("load {ll}, ptr {b}"));
                self.assign(format!("fcmp oeq {ll} {a}, {b}"))
            }
            Shape::Prim(_) => {
                let ll = self.ll(ty);
                let a = self.assign(format!("load {ll}, ptr {a}"));
                let b = self.assign(format!("load {ll}, ptr {b}"));
                self.assign(format!("icmp eq {ll} {a}, {b}"))
            }
            _ => self.call_equals(a, b, ty),
        }
    }

    fn field_hash(&mut self, a: &str, ty: &Ty) -> String {
        match self.cx.shape(ty) {
            Shape::Prim(primitive) => {
                let value = self.load(ty, a);
                self.prim_hash(&value, primitive)
            }
            _ => self.call_hash(a, ty),
        }
    }

    fn prim_hash(&mut self, value: &str, primitive: PrimitiveTy) -> String {
        match primitive {
            PrimitiveTy::Boolean => self.assign(format!("select i1 {value}, i32 1231, i32 1237")),
            PrimitiveTy::Byte | PrimitiveTy::Short => self.assign(format!(
                "sext {} {value} to i32",
                crate::layout::prim_llty(primitive)
            )),
            PrimitiveTy::Char | PrimitiveTy::Int => value.to_owned(),
            PrimitiveTy::Long => {
                let shifted = self.assign(format!("lshr i64 {value}, 32"));
                let mixed = self.assign(format!("xor i64 {value}, {shifted}"));
                self.assign(format!("trunc i64 {mixed} to i32"))
            }
            PrimitiveTy::Float => self.assign(format!("bitcast float {value} to i32")),
            PrimitiveTy::Double => {
                let bits = self.assign(format!("bitcast double {value} to i64"));
                let shifted = self.assign(format!("lshr i64 {bits}, 32"));
                let mixed = self.assign(format!("xor i64 {bits}, {shifted}"));
                self.assign(format!("trunc i64 {mixed} to i32"))
            }
        }
    }

    fn checked_string(&mut self, value: &str, item: LangItem, message: &str) -> String {
        let failed = self.assign(format!("icmp eq ptr {value}, null"));
        let fail = self.fail_label();
        let ok = self.label("ok");
        let bad = self.label("bad");
        self.line(format!("br i1 {failed}, label %{bad}, label %{ok}"));
        self.start(&bad);
        self.throw_new(item, message, &fail);
        self.line("unreachable");
        self.start(&ok);
        value.to_owned()
    }

    fn string_builtin(&mut self, name: &str, builtin: &Builtin) -> bool {
        let string = self.cx.tcx.string_ty();
        let arg_string = |fx: &mut Self, index: usize| -> String {
            let ty = builtin.arg_ty(index);
            let arg = builtin.arg(index);
            match ty {
                Ty::Ref { .. } => fx.assign(format!("load ptr, ptr {arg}")),
                _ => arg,
            }
        };
        let release_args = |fx: &mut Self| {
            let offset = usize::from(builtin.receiver.is_some());
            for index in offset..builtin.params.len() {
                let ty = builtin.params[index].clone();
                if fx.cx.needs_drop(&ty) {
                    fx.drop_value(&format!("%a{index}"), &ty);
                }
            }
        };
        if name == "valueOf" {
            let text = self.stringify("%a0", &builtin.params[0]);
            self.drop_value("%a0", &builtin.params[0].clone());
            self.ret(&text, &string);
            return true;
        }
        if name == "join" {
            let joined = self.runtime("ptr", "kr_string_join", "ptr %a0, ptr %a1");
            release_args(self);
            self.ret(&joined, &string);
            return true;
        }
        let this = self.this_value(builtin);
        let result: (String, Ty) = match name {
            "length" => (
                self.runtime("i32", "kr_string_length", &format!("ptr {this}")),
                Ty::INT,
            ),
            "chars" => {
                let array_ty = Ty::Array(Box::new(Ty::CHAR));
                let descriptor = self.cx.rt_global(RtKey::Array(array_ty.clone()));
                self.cx.drop_array_fn(&array_ty);
                (
                    self.runtime(
                        "ptr",
                        "kr_string_chars",
                        &format!("ptr {this}, ptr {descriptor}"),
                    ),
                    array_ty,
                )
            }
            "substring" => {
                let begin = builtin.arg(0);
                let end = if builtin.params.len() > 2 {
                    builtin.arg(1)
                } else {
                    self.runtime("i32", "kr_string_length", &format!("ptr {this}"))
                };
                let value = self.runtime(
                    "ptr",
                    "kr_string_substring",
                    &format!("ptr {this}, i32 {begin}, i32 {end}"),
                );
                let value = self.checked_string(
                    &value,
                    LangItem::IndexOutOfBoundsException,
                    "substring range is out of bounds",
                );
                (value, string.clone())
            }
            "indexOf" => {
                let text = arg_string(self, 0);
                (
                    self.runtime(
                        "i32",
                        "kr_string_index_of",
                        &format!("ptr {this}, ptr {text}"),
                    ),
                    Ty::INT,
                )
            }
            "startsWith" | "endsWith" | "equals" => {
                let other = arg_string(self, 0);
                let function = match name {
                    "startsWith" => "kr_string_starts_with",
                    "endsWith" => "kr_string_ends_with",
                    _ => "kr_string_equals",
                };
                (
                    self.runtime("i1", function, &format!("ptr {this}, ptr {other}")),
                    Ty::BOOLEAN,
                )
            }
            "compareTo" => {
                let other = arg_string(self, 0);
                (
                    self.runtime(
                        "i32",
                        "kr_string_compare",
                        &format!("ptr {this}, ptr {other}"),
                    ),
                    Ty::INT,
                )
            }
            "hashCode" => (
                self.runtime("i32", "kr_string_hash", &format!("ptr {this}")),
                Ty::INT,
            ),
            "toUpperCase" => (
                self.runtime("ptr", "kr_string_to_upper", &format!("ptr {this}")),
                string.clone(),
            ),
            "toLowerCase" => (
                self.runtime("ptr", "kr_string_to_lower", &format!("ptr {this}")),
                string.clone(),
            ),
            "trim" => (
                self.runtime("ptr", "kr_string_trim", &format!("ptr {this}")),
                string.clone(),
            ),
            "replace" => {
                let target = arg_string(self, 0);
                let replacement = arg_string(self, 1);
                (
                    self.runtime(
                        "ptr",
                        "kr_string_replace",
                        &format!("ptr {this}, ptr {target}, ptr {replacement}"),
                    ),
                    string.clone(),
                )
            }
            "repeat" => {
                let count = builtin.arg(0);
                let value = self.runtime(
                    "ptr",
                    "kr_string_repeat",
                    &format!("ptr {this}, i32 {count}"),
                );
                let value = self.checked_string(
                    &value,
                    LangItem::IllegalArgumentException,
                    "repeat count is negative",
                );
                (value, string.clone())
            }
            "split" => {
                let separator = arg_string(self, 0);
                let array_ty = Ty::Array(Box::new(string.clone()));
                let descriptor = self.cx.rt_global(RtKey::Array(array_ty.clone()));
                self.cx.drop_array_fn(&array_ty);
                (
                    self.runtime(
                        "ptr",
                        "kr_string_split",
                        &format!("ptr {this}, ptr {separator}, ptr {descriptor}"),
                    ),
                    array_ty,
                )
            }
            _ => return false,
        };
        release_args(self);
        if builtin.receiver == Some(ReceiverKind::Owned) {
            self.drop_value("%a0", &string);
        }
        self.ret(&result.0, &result.1);
        true
    }

    fn console_builtin(&mut self, name: &str, builtin: &Builtin) -> bool {
        let string = self.cx.tcx.string_ty();
        match name {
            "print" | "println" => {
                if !builtin.params.is_empty() {
                    let ty = builtin.params[0].clone();
                    let text = self.stringify("%a0", &ty);
                    self.runtime("void", "kr_console_print", &format!("ptr {text}"));
                    self.drop_value(&text, &string);
                    self.drop_value("%a0", &ty);
                }
                if name == "println" {
                    self.runtime("void", "kr_console_newline", "");
                }
                self.line("ret void");
                true
            }
            "readLine" => {
                let line = self.runtime("ptr", "kr_console_read_line", "");
                let ret = self.ret_ty.clone();
                self.ret(&line, &ret);
                true
            }
            _ => false,
        }
    }

    fn math_builtin(&mut self, name: &str, builtin: &Builtin) -> bool {
        let unary = |fx: &mut Self, intrinsic: &str| -> String {
            fx.cx
                .declare_dynamic(intrinsic, &format!("declare double @{intrinsic}(double)"));
            fx.assign(format!("call double @{intrinsic}(double %a0)"))
        };
        let value = match name {
            "sqrt" => unary(self, "llvm.sqrt.f64"),
            "floor" => unary(self, "llvm.floor.f64"),
            "ceil" => unary(self, "llvm.ceil.f64"),
            "pow" => {
                self.cx.declare_dynamic(
                    "llvm.pow.f64",
                    "declare double @llvm.pow.f64(double, double)",
                );
                self.assign("call double @llvm.pow.f64(double %a0, double %a1)")
            }
            "round" => {
                let shifted = self.assign(format!("fadd double %a0, {}", float_const(0.5)));
                self.cx
                    .declare_dynamic("llvm.floor.f64", "declare double @llvm.floor.f64(double)");
                let floored = self.assign(format!("call double @llvm.floor.f64(double {shifted})"));
                let value = self.numeric(&floored, PrimitiveTy::Double, PrimitiveTy::Long);
                self.ret(&value, &Ty::LONG);
                return true;
            }
            "random" => self.runtime("double", "kr_random", ""),
            _ => return false,
        };
        let _ = builtin;
        self.ret(&value, &Ty::DOUBLE);
        true
    }

    fn primitive_builtin(&mut self, primitive: PrimitiveTy, name: &str, builtin: &Builtin) -> bool {
        match name {
            "hashCode" => {
                let value = self.load(&Ty::Primitive(primitive), "%a0");
                let hash = self.prim_hash(&value, primitive);
                self.ret(&hash, &Ty::INT);
                true
            }
            "parse" => {
                let string = self.cx.tcx.string_ty();
                let ty = Ty::Primitive(primitive);
                let fail = self.fail_label();
                let (ok, value) = match primitive {
                    PrimitiveTy::Float | PrimitiveTy::Double => {
                        let out = self.alloca_ll("double");
                        let ok =
                            self.runtime("i1", "kr_parse_double", &format!("ptr %a0, ptr {out}"));
                        let value = self.assign(format!("load double, ptr {out}"));
                        let value = self.numeric(&value, PrimitiveTy::Double, primitive);
                        (ok, value)
                    }
                    _ => {
                        let (min, max) = match primitive {
                            PrimitiveTy::Byte => (i8::MIN as i64, i8::MAX as i64),
                            PrimitiveTy::Short => (i16::MIN as i64, i16::MAX as i64),
                            PrimitiveTy::Int => (i32::MIN as i64, i32::MAX as i64),
                            _ => (i64::MIN, i64::MAX),
                        };
                        let out = self.alloca_ll("i64");
                        let ok = self.runtime(
                            "i1",
                            "kr_parse_long",
                            &format!("ptr %a0, i64 {min}, i64 {max}, ptr {out}"),
                        );
                        let value = self.assign(format!("load i64, ptr {out}"));
                        let value = self.numeric(&value, PrimitiveTy::Long, primitive);
                        (ok, value)
                    }
                };
                let good = self.label("parsed");
                let bad = self.label("unparsed");
                self.line(format!("br i1 {ok}, label %{good}, label %{bad}"));
                self.start(&bad);
                let message = self.runtime("ptr", "kr_string_number_format", "ptr %a0");
                self.drop_value("%a0", &string);
                self.throw_with(LangItem::NumberFormatException, &message, &fail);
                self.line("unreachable");
                self.start(&good);
                self.drop_value("%a0", &string);
                self.ret(&value, &ty);
                true
            }
            _ if primitive == PrimitiveTy::Char => {
                let value = self.load(&Ty::CHAR, "%a0");
                let (function, ret) = match name {
                    "isDigit" => ("kr_char_is_digit", Ty::BOOLEAN),
                    "isLetter" => ("kr_char_is_letter", Ty::BOOLEAN),
                    "isWhitespace" => ("kr_char_is_whitespace", Ty::BOOLEAN),
                    "isUpperCase" => ("kr_char_is_upper", Ty::BOOLEAN),
                    "isLowerCase" => ("kr_char_is_lower", Ty::BOOLEAN),
                    "toUpperCase" => ("kr_char_to_upper", Ty::CHAR),
                    "toLowerCase" => ("kr_char_to_lower", Ty::CHAR),
                    "isLetterOrDigit" => {
                        let letter =
                            self.runtime("i1", "kr_char_is_letter", &format!("i32 {value}"));
                        let digit = self.runtime("i1", "kr_char_is_digit", &format!("i32 {value}"));
                        let either = self.assign(format!("or i1 {letter}, {digit}"));
                        self.ret(&either, &Ty::BOOLEAN);
                        return true;
                    }
                    _ => return false,
                };
                let ll = self.ll(&ret);
                let result = self.runtime(&ll, function, &format!("i32 {value}"));
                self.ret(&result, &ret);
                let _ = builtin;
                true
            }
            _ => false,
        }
    }

    fn thread_builtin(&mut self, name: &str, builtin: &Builtin) -> bool {
        match name {
            "sleep" => {
                self.runtime("void", "kr_thread_sleep", "i64 %a0");
                self.line("ret void");
                true
            }
            "start" | "join" => {
                let ty = builtin.self_ty.clone();
                let object = self.assign("load ptr, ptr %a0");
                let handle_addr = self.native_addr(&object, &ty, 0);
                if name == "start" {
                    let Ty::Adt { def, args, .. } = &ty else {
                        return false;
                    };
                    let task = krelpin_span::Symbol::intern("task");
                    let index = self.cx.field_index(&ty, *def, task);
                    let runnable = self.cx.field_ty(*def, args, task);
                    let object_ll = self.cx.object_type(&ty);
                    let task_addr = self.assign(format!(
                        "getelementptr inbounds {object_ll}, ptr {object}, i32 0, i32 {index}"
                    ));
                    let entry = self
                        .cx
                        .instance(Instance::Thunk(Thunk::ThreadEntry { runnable }));
                    let entry = self.cx.fn_name(entry);
                    let previous = self.assign(format!("load ptr, ptr {handle_addr}"));
                    self.runtime("void", "kr_thread_join", &format!("ptr {previous}"));
                    let handle = self.runtime(
                        "ptr",
                        "kr_thread_spawn",
                        &format!("ptr {entry}, ptr {task_addr}"),
                    );
                    self.line(format!("store ptr {handle}, ptr {handle_addr}"));
                } else {
                    let handle = self.assign(format!("load ptr, ptr {handle_addr}"));
                    self.runtime("void", "kr_thread_join", &format!("ptr {handle}"));
                    self.line(format!("store ptr null, ptr {handle_addr}"));
                }
                self.line("ret void");
                true
            }
            _ => false,
        }
    }

    fn atomic_builtin(&mut self, owner: DefId, name: &str, builtin: &Builtin) -> bool {
        let ty = builtin.self_ty.clone();
        let value_name = krelpin_span::Symbol::intern("value");
        let value_ty = self.cx.field_ty(owner, &[], value_name);
        let object = self.assign("load ptr, ptr %a0");
        let object_ll = self.cx.object_type(&ty);
        let index = self.cx.field_index(&ty, owner, value_name);
        let address = self.assign(format!(
            "getelementptr inbounds {object_ll}, ptr {object}, i32 0, i32 {index}"
        ));
        let boolean = value_ty == Ty::BOOLEAN;
        let ll = if boolean {
            "i8".to_owned()
        } else {
            self.ll(&value_ty)
        };
        let widen = |fx: &mut Self, value: &str| -> String {
            if boolean {
                fx.assign(format!("zext i1 {value} to i8"))
            } else {
                value.to_owned()
            }
        };
        let narrow = |fx: &mut Self, value: &str| -> String {
            if boolean {
                fx.assign(format!("trunc i8 {value} to i1"))
            } else {
                value.to_owned()
            }
        };
        match name {
            "get" => {
                let value =
                    self.assign(format!("load atomic {ll}, ptr {address} seq_cst, align 8"));
                let value = narrow(self, &value);
                self.ret(&value, &value_ty);
            }
            "set" => {
                let value = widen(self, &builtin.arg(0));
                self.line(format!(
                    "store atomic {ll} {value}, ptr {address} seq_cst, align 8"
                ));
                self.line("ret void");
            }
            "compareAndSet" => {
                let expected = widen(self, &builtin.arg(0));
                let value = widen(self, &builtin.arg(1));
                let pair = self.assign(format!(
                    "cmpxchg ptr {address}, {ll} {expected}, {ll} {value} seq_cst seq_cst"
                ));
                let swapped = self.assign(format!("extractvalue {{ {ll}, i1 }} {pair}, 1"));
                self.ret(&swapped, &Ty::BOOLEAN);
            }
            "increment" | "decrement" | "addAndGet" => {
                let delta = match name {
                    "increment" => "1".to_owned(),
                    "decrement" => "-1".to_owned(),
                    _ => builtin.arg(0),
                };
                let old = self.assign(format!("atomicrmw add ptr {address}, {ll} {delta} seq_cst"));
                let new = self.assign(format!("add {ll} {old}, {delta}"));
                self.ret(&new, &value_ty);
            }
            _ => return false,
        }
        true
    }

    fn mutex_builtin(&mut self, owner: DefId, name: &str, builtin: &Builtin) -> bool {
        let ty = builtin.self_ty.clone();
        let items = &self.cx.tcx.lang_items;
        if items.is(owner, LangItem::Mutex) && name == "lock" {
            let object = self.assign("load ptr, ptr %a0");
            let handle = self.native_addr(&object, &ty, 0);
            let handle = self.assign(format!("load ptr, ptr {handle}"));
            self.runtime("void", "kr_mutex_lock", &format!("ptr {handle}"));
            let guard_ty = self.ret_ty.clone();
            let guard = self.new_object(&guard_ty, None);
            let slot = self.native_addr(&guard, &guard_ty, 0);
            self.line(format!("store ptr {object}, ptr {slot}"));
            self.ret(&guard, &guard_ty);
            return true;
        }
        if items.is(owner, LangItem::MutexGuard) && matches!(name, "get" | "getMut") {
            let Ty::Adt { args, .. } = &ty else {
                return false;
            };
            let guard = self.assign("load ptr, ptr %a0");
            let mutex = self.native_addr(&guard, &ty, 0);
            let mutex = self.assign(format!("load ptr, ptr {mutex}"));
            let mutex_def = self.cx.lang(LangItem::Mutex);
            let mutex_ty = Ty::Adt {
                def: mutex_def,
                name: self.cx.tcx.sig(mutex_def).name.name,
                args: args.clone(),
            };
            let value = krelpin_span::Symbol::intern("value");
            let index = self.cx.field_index(&mutex_ty, mutex_def, value);
            let object_ll = self.cx.object_type(&mutex_ty);
            let address = self.assign(format!(
                "getelementptr inbounds {object_ll}, ptr {mutex}, i32 0, i32 {index}"
            ));
            self.line(format!("ret ptr {address}"));
            return true;
        }
        false
    }

    fn list_builtin(&mut self, name: &str, builtin: &Builtin) -> bool {
        let ty = builtin.self_ty.clone();
        let Ty::Adt { args, .. } = &ty else {
            return false;
        };
        let elem = args[0].clone();
        let elem_ll = self.cx.mem_llty(&elem);
        let (elem_size, _) = self.cx.size_align(&elem);
        let object = self.assign("load ptr, ptr %a0");
        let data_addr = self.native_addr(&object, &ty, 0);
        let len_addr = self.native_addr(&object, &ty, 1);
        let cap_addr = self.native_addr(&object, &ty, 2);
        let len = self.assign(format!("load i64, ptr {len_addr}"));
        let fail = self.fail_label();
        let check_index = |fx: &mut Self, index: &str, inclusive: bool| -> String {
            let wide = fx.assign(format!("sext i32 {index} to i64"));
            let op = if inclusive { "ule" } else { "ult" };
            let inside = fx.assign(format!("icmp {op} i64 {wide}, {len}"));
            let ok = fx.label("inbounds");
            let bad = fx.label("outofbounds");
            fx.line(format!("br i1 {inside}, label %{ok}, label %{bad}"));
            fx.start(&bad);
            let message = fx.runtime("ptr", "kr_string_bounds", &format!("i64 {wide}, i64 {len}"));
            fx.throw_with(LangItem::IndexOutOfBoundsException, &message, &fail);
            fx.line("unreachable");
            fx.start(&ok);
            wide
        };
        let grow = |fx: &mut Self| {
            let cap = fx.assign(format!("load i64, ptr {cap_addr}"));
            let full = fx.assign(format!("icmp eq i64 {len}, {cap}"));
            let more = fx.label("grow");
            let room = fx.label("room");
            fx.line(format!("br i1 {full}, label %{more}, label %{room}"));
            fx.start(&more);
            let doubled = fx.assign(format!("mul i64 {cap}, 2"));
            let small = fx.assign(format!("icmp slt i64 {doubled}, 4"));
            let new_cap = fx.assign(format!("select i1 {small}, i64 4, i64 {doubled}"));
            let bytes = fx.assign(format!("mul i64 {new_cap}, {elem_size}"));
            let data = fx.assign(format!("load ptr, ptr {data_addr}"));
            let grown = fx.runtime("ptr", "kr_realloc", &format!("ptr {data}, i64 {bytes}"));
            fx.line(format!("store ptr {grown}, ptr {data_addr}"));
            fx.line(format!("store i64 {new_cap}, ptr {cap_addr}"));
            fx.line(format!("br label %{room}"));
            fx.start(&room);
        };
        let slot = |fx: &mut Self, index: &str| -> String {
            let data = fx.assign(format!("load ptr, ptr {data_addr}"));
            fx.assign(format!(
                "getelementptr inbounds {elem_ll}, ptr {data}, i64 {index}"
            ))
        };
        let shift = |fx: &mut Self, from: &str, to: &str, count: &str| {
            let source = slot(fx, from);
            let target = slot(fx, to);
            let bytes = fx.assign(format!("mul i64 {count}, {elem_size}"));
            fx.cx.declare_dynamic(
                "llvm.memmove.p0.p0.i64",
                "declare void @llvm.memmove.p0.p0.i64(ptr, ptr, i64, i1)",
            );
            fx.line(format!(
                "call void @llvm.memmove.p0.p0.i64(ptr {target}, ptr {source}, i64 {bytes}, i1 0)"
            ));
        };
        match name {
            "size" => {
                let value = self.assign(format!("trunc i64 {len} to i32"));
                self.ret(&value, &Ty::INT);
            }
            "get" => {
                let index = check_index(self, &builtin.arg(0), false);
                let address = slot(self, &index);
                self.line(format!("ret ptr {address}"));
            }
            "set" => {
                let index = check_index(self, &builtin.arg(0), false);
                let address = slot(self, &index);
                let old = self.load(&elem, &address);
                let value = self.convert(&builtin.arg(1), &builtin.arg_ty(1), &elem);
                self.store(&elem, &value, &address);
                self.ret(&old, &elem);
            }
            "add" if builtin.params.len() == 2 => {
                grow(self);
                let address = slot(self, &len);
                let value = self.convert(&builtin.arg(0), &builtin.arg_ty(0), &elem);
                self.store(&elem, &value, &address);
                let next = self.assign(format!("add i64 {len}, 1"));
                self.line(format!("store i64 {next}, ptr {len_addr}"));
                self.line("ret void");
            }
            "add" => {
                let index = check_index(self, &builtin.arg(0), true);
                grow(self);
                let after = self.assign(format!("add i64 {index}, 1"));
                let count = self.assign(format!("sub i64 {len}, {index}"));
                shift(self, &index, &after, &count);
                let address = slot(self, &index);
                let value = self.convert(&builtin.arg(1), &builtin.arg_ty(1), &elem);
                self.store(&elem, &value, &address);
                let next = self.assign(format!("add i64 {len}, 1"));
                self.line(format!("store i64 {next}, ptr {len_addr}"));
                self.line("ret void");
            }
            "remove" => {
                let index = check_index(self, &builtin.arg(0), false);
                let address = slot(self, &index);
                let old = self.load(&elem, &address);
                let after = self.assign(format!("add i64 {index}, 1"));
                let last = self.assign(format!("sub i64 {len}, 1"));
                let count = self.assign(format!("sub i64 {last}, {index}"));
                shift(self, &after, &index, &count);
                self.line(format!("store i64 {last}, ptr {len_addr}"));
                self.ret(&old, &elem);
            }
            "clear" => {
                self.drop_list_elements(&object, &ty, &elem);
                self.line("ret void");
            }
            "indexOf" => {
                let counter = self.alloca_ll("i64");
                let head = self.label("search");
                let body = self.label("compare");
                let found = self.label("found");
                let next = self.label("next");
                let missing = self.label("missing");
                self.line(format!("br label %{head}"));
                self.start(&head);
                let index = self.assign(format!("load i64, ptr {counter}"));
                let more = self.assign(format!("icmp slt i64 {index}, {len}"));
                self.line(format!("br i1 {more}, label %{body}, label %{missing}"));
                self.start(&body);
                let address = slot(self, &index);
                let target = builtin.arg(0);
                let equal = self.call_equals(&address, &target, &elem);
                self.line(format!("br i1 {equal}, label %{found}, label %{next}"));
                self.start(&next);
                let following = self.assign(format!("add i64 {index}, 1"));
                self.line(format!("store i64 {following}, ptr {counter}"));
                self.line(format!("br label %{head}"));
                self.start(&found);
                let narrow = self.assign(format!("trunc i64 {index} to i32"));
                self.line(format!("ret i32 {narrow}"));
                self.start(&missing);
                self.line("ret i32 -1");
            }
            _ => return false,
        }
        let _ = Val {
            ty: Ty::Void,
            v: String::new(),
        };
        true
    }

    fn table_builtin(&mut self, owner: DefId, name: &str, builtin: &Builtin) -> bool {
        let ty = builtin.self_ty.clone();
        let Ty::Adt { args, .. } = &ty else {
            return false;
        };
        let is_map = self.cx.tcx.lang_items.is(owner, LangItem::HashMap);
        let key = args[0].clone();
        let value = if is_map { args[1].clone() } else { Ty::Void };
        let shape = self.cx.table_shape(&key, &value);
        let entry_ll = self.cx.entry_type(&key, &value);
        let object = self.assign("load ptr, ptr %a0");
        let table = self.native_addr(&object, &ty, 0);
        let ret = self.ret_ty.clone();
        match name {
            "size" => {
                let live = self.native_addr(&object, &ty, 3);
                let live = self.assign(format!("load i64, ptr {live}"));
                let value = self.assign(format!("trunc i64 {live} to i32"));
                self.ret(&value, &Ty::INT);
            }
            "get" | "containsKey" | "contains" => {
                let entry = self.runtime(
                    "ptr",
                    "kr_table_find",
                    &format!("ptr {table}, ptr {shape}, ptr %a1"),
                );
                if name == "get" {
                    let is_null = self.assign(format!("icmp eq ptr {entry}, null"));
                    let address = self.assign(format!(
                        "getelementptr inbounds {entry_ll}, ptr {entry}, i32 0, i32 3"
                    ));
                    let result =
                        self.assign(format!("select i1 {is_null}, ptr null, ptr {address}"));
                    self.line(format!("ret ptr {result}"));
                } else {
                    let found = self.assign(format!("icmp ne ptr {entry}, null"));
                    self.ret(&found, &Ty::BOOLEAN);
                }
            }
            "put" | "add" => {
                let temp = self.alloca_ll(&entry_ll);
                let key_addr = self.assign(format!(
                    "getelementptr inbounds {entry_ll}, ptr {temp}, i32 0, i32 2"
                ));
                let key_value = self.convert("%a1", &builtin.params[1].clone(), &key);
                self.store(&key, &key_value, &key_addr);
                if is_map {
                    let value_addr = self.assign(format!(
                        "getelementptr inbounds {entry_ll}, ptr {temp}, i32 0, i32 3"
                    ));
                    let stored = self.convert("%a2", &builtin.params[2].clone(), &value);
                    self.store(&value, &stored, &value_addr);
                }
                let existed = self.alloca_ll("i1");
                self.runtime(
                    "ptr",
                    "kr_table_insert",
                    &format!("ptr {table}, ptr {shape}, ptr {temp}, ptr {existed}"),
                );
                let existed = self.assign(format!("load i1, ptr {existed}"));
                let fail = self.fail_label();
                let pending = self.assign("load ptr, ptr @kr_pending");
                let failed = self.assign(format!("icmp ne ptr {pending}, null"));
                let fine = self.label("inserted");
                let undo = self.label("undo");
                self.line(format!("br i1 {failed}, label %{undo}, label %{fine}"));
                self.start(&undo);
                self.drop_at(&key, &key_addr);
                if is_map {
                    let value_addr = self.assign(format!(
                        "getelementptr inbounds {entry_ll}, ptr {temp}, i32 0, i32 3"
                    ));
                    self.drop_at(&value, &value_addr);
                }
                self.line(format!("br label %{fail}"));
                self.start(&fine);
                let replaced = self.label("replaced");
                let fresh = self.label("fresh");
                self.line(format!(
                    "br i1 {existed}, label %{replaced}, label %{fresh}"
                ));
                self.start(&replaced);
                self.drop_at(&key, &key_addr);
                if is_map {
                    let value_addr = self.assign(format!(
                        "getelementptr inbounds {entry_ll}, ptr {temp}, i32 0, i32 3"
                    ));
                    let old = self.load(&value, &value_addr);
                    self.zero(&value, &value_addr);
                    self.ret(&old, &value);
                } else {
                    self.ret("false", &Ty::BOOLEAN);
                }
                self.start(&fresh);
                self.zero(&key, &key_addr);
                if is_map {
                    let none = self.cx.null_value(&ret);
                    let ll = self.ll(&ret);
                    self.line(format!("ret {ll} {none}"));
                } else {
                    self.ret("true", &Ty::BOOLEAN);
                }
            }
            "remove" => {
                let temp = self.alloca_ll(&entry_ll);
                let removed = self.runtime(
                    "i1",
                    "kr_table_remove",
                    &format!("ptr {table}, ptr {shape}, ptr %a1, ptr {temp}"),
                );
                let gone = self.label("removed");
                let kept = self.label("absent");
                self.line(format!("br i1 {removed}, label %{gone}, label %{kept}"));
                self.start(&gone);
                let key_addr = self.assign(format!(
                    "getelementptr inbounds {entry_ll}, ptr {temp}, i32 0, i32 2"
                ));
                self.drop_at(&key, &key_addr);
                if is_map {
                    let value_addr = self.assign(format!(
                        "getelementptr inbounds {entry_ll}, ptr {temp}, i32 0, i32 3"
                    ));
                    let old = self.load(&value, &value_addr);
                    self.zero(&value, &value_addr);
                    self.ret(&old, &value);
                } else {
                    self.ret("true", &Ty::BOOLEAN);
                }
                self.start(&kept);
                let none = self.cx.null_value(&ret);
                let ll = self.ll(&ret);
                self.line(format!("ret {ll} {none}"));
            }
            "clear" => {
                self.runtime(
                    "void",
                    "kr_table_clear",
                    &format!("ptr {table}, ptr {shape}"),
                );
                self.line("ret void");
            }
            "slotCount" => {
                let count = self.runtime("i64", "kr_table_slot_count", &format!("ptr {table}"));
                let count = self.assign(format!("trunc i64 {count} to i32"));
                self.ret(&count, &Ty::INT);
            }
            "slotAt" => {
                let index = self.assign("sext i32 %a1 to i64");
                let entry = self.runtime(
                    "ptr",
                    "kr_table_slot",
                    &format!("ptr {table}, ptr {shape}, i64 {index}"),
                );
                let is_null = self.assign(format!("icmp eq ptr {entry}, null"));
                let address = self.assign(format!(
                    "getelementptr inbounds {entry_ll}, ptr {entry}, i32 0, i32 2"
                ));
                let result = self.assign(format!("select i1 {is_null}, ptr null, ptr {address}"));
                self.line(format!("ret ptr {result}"));
            }
            "keySet" | "values" => {
                let projected = if name == "keySet" {
                    key.clone()
                } else {
                    value.clone()
                };
                let reference = Ty::Ref {
                    mutable: false,
                    ty: Box::new(projected.clone()),
                };
                let item = if name == "keySet" {
                    LangItem::HashSet
                } else {
                    LangItem::ArrayList
                };
                let collection_def = self.cx.lang(item);
                let collection_ty = Ty::Adt {
                    def: collection_def,
                    name: self.cx.tcx.sig(collection_def).name.name,
                    args: vec![reference.clone()],
                };
                let collection = self.new_object(&collection_ty, None);
                let count = self.runtime("i64", "kr_table_slot_count", &format!("ptr {table}"));
                let counter = self.alloca_ll("i64");
                let head = self.label("walk");
                let body = self.label("visit");
                let live = self.label("live");
                let next = self.label("advance");
                let done = self.label("walked");
                self.line(format!("br label %{head}"));
                self.start(&head);
                let index = self.assign(format!("load i64, ptr {counter}"));
                let more = self.assign(format!("icmp slt i64 {index}, {count}"));
                self.line(format!("br i1 {more}, label %{body}, label %{done}"));
                self.start(&body);
                let entry = self.runtime(
                    "ptr",
                    "kr_table_slot",
                    &format!("ptr {table}, ptr {shape}, i64 {index}"),
                );
                let present = self.assign(format!("icmp ne ptr {entry}, null"));
                self.line(format!("br i1 {present}, label %{live}, label %{next}"));
                self.start(&live);
                let field = if name == "keySet" { 2 } else { 3 };
                let address = self.assign(format!(
                    "getelementptr inbounds {entry_ll}, ptr {entry}, i32 0, i32 {field}"
                ));
                let slot = self.alloca_ll("ptr");
                self.line(format!("store ptr {collection}, ptr {slot}"));
                let add = self.collection_add(&collection_ty, "add");
                let name_fn = self.cx.fn_name(add);
                if name == "keySet" {
                    self.line(format!("call i1 {name_fn}(ptr {slot}, ptr {address})"));
                } else {
                    self.line(format!("call void {name_fn}(ptr {slot}, ptr {address})"));
                }
                self.line(format!("br label %{next}"));
                self.start(&next);
                let following = self.assign(format!("add i64 {index}, 1"));
                self.line(format!("store i64 {following}, ptr {counter}"));
                self.line(format!("br label %{head}"));
                self.start(&done);
                self.ret(&collection, &collection_ty);
            }
            _ => return false,
        }
        true
    }

    fn collection_add(&mut self, collection: &Ty, name: &str) -> usize {
        let Ty::Adt { def, .. } = collection else {
            panic!("`{collection}` is not a collection");
        };
        let method = self
            .cx
            .tcx
            .sig(*def)
            .methods
            .get(&krelpin_span::Symbol::intern(name))
            .and_then(|methods| methods.iter().find(|method| method.params.len() == 1))
            .cloned()
            .unwrap_or_else(|| panic!("`{collection}` declares `{name}`"));
        let subst = match collection {
            Ty::Adt { def, args, .. } => self.cx.tcx.subst_for(*def, args),
            _ => Default::default(),
        };
        let instance = self.cx.instance_for(&method, &subst, collection);
        self.cx.instance(instance)
    }
}

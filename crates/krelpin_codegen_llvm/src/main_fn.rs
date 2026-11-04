use krelpin_mir::BodyId;
use krelpin_typeck::{LangItem, Subst, Ty};

use crate::cx::Cx;
use crate::func::FnCx;
use crate::instance::Instance;
use crate::rtti::RtKey;

pub enum Entry {
    Main(BodyId),
    Tests(Vec<TestCase>),
    Library {
        prefix: String,
        exports: Vec<Export>,
    },
}

pub struct TestCase {
    pub body: BodyId,
    pub name: String,
}

pub struct Export {
    pub body: BodyId,
    pub symbol: String,
}

pub(crate) enum Compiled {
    Main {
        id: usize,
        takes_args: bool,
    },
    Tests(Vec<(usize, String)>),
    Library {
        prefix: String,
        exports: Vec<(usize, String)>,
    },
}

impl Cx<'_, '_> {
    pub(crate) fn prepare_entry(&mut self, entry: &Entry) {
        let body = |cx: &mut Self, body: BodyId| {
            cx.instance(Instance::Body {
                body,
                args: Vec::new(),
            })
        };
        let compiled = match entry {
            Entry::Main(main) => Compiled::Main {
                id: body(self, *main),
                takes_args: self.mir.body(*main).arg_count == 1,
            },
            Entry::Tests(tests) => Compiled::Tests(
                tests
                    .iter()
                    .map(|test| (body(self, test.body), test.name.clone()))
                    .collect(),
            ),
            Entry::Library { prefix, exports } => Compiled::Library {
                prefix: prefix.clone(),
                exports: exports
                    .iter()
                    .map(|export| (body(self, export.body), export.symbol.clone()))
                    .collect(),
            },
        };
        if let Compiled::Main {
            takes_args: true, ..
        } = compiled
        {
            let array = Ty::Array(Box::new(self.tcx.string_ty()));
            self.rt_key(RtKey::Array(array.clone()));
            self.drop_array_fn(&array);
        }
        self.entry = Some(compiled);
        self.message_fn = Some(self.message_instance());
        let throwable = self.lang_ty(LangItem::Throwable);
        self.drop_glue(&throwable);
    }

    fn static_order(&self) -> Vec<usize> {
        let mut order = Vec::new();
        let mut state = vec![0u8; self.statics.len()];
        for start in 0..self.statics.len() {
            self.visit_static(start, &mut state, &mut order);
        }
        order
    }

    fn visit_static(&self, id: usize, state: &mut [u8], order: &mut Vec<usize>) {
        if state[id] != 0 {
            return;
        }
        state[id] = 1;
        for &dep in &self.statics[id].deps {
            self.visit_static(dep, state, order);
        }
        state[id] = 2;
        order.push(id);
    }

    fn static_inits(&self) -> Vec<(String, Ty, String)> {
        self.static_order()
            .iter()
            .filter_map(|&id| {
                let info = &self.statics[id];
                let init = info.init?;
                Some((self.fn_name(init), info.ty.clone(), info.global.clone()))
            })
            .collect()
    }

    pub(crate) fn emit_entry(&mut self) -> String {
        match self.entry.take().expect("an entry point is compiled") {
            Compiled::Main { id, takes_args } => self.emit_main(id, takes_args),
            Compiled::Tests(tests) => self.emit_test_main(&tests),
            Compiled::Library { prefix, exports } => self.emit_library(&prefix, &exports),
        }
    }

    fn emit_main(&mut self, entry: usize, takes_args: bool) -> String {
        let entry_name = self.fn_name(entry);
        let inits = self.static_inits();
        let args_descriptor = takes_args.then(|| {
            let array = Ty::Array(Box::new(self.tcx.string_ty()));
            self.rt_global(RtKey::Array(array))
        });
        let message = self.fn_name(self.message_fn.expect("the uncaught handler is prepared"));
        let mut fx = FnCx::new(self, Subst::new(), Ty::INT);
        fx.start("entry");
        fx.runtime("void", "kr_runtime_init", "i32 %argc, ptr %argv");
        fx.run_static_inits(&inits, "uncaught");
        match args_descriptor {
            Some(descriptor) => {
                let args = fx.runtime("ptr", "kr_runtime_args", &format!("ptr {descriptor}"));
                fx.line(format!("call void {entry_name}(ptr {args})"));
            }
            None => fx.line(format!("call void {entry_name}()")),
        }
        fx.check_pending("uncaught");
        fx.runtime("void", "kr_runtime_exit", "");
        fx.line("ret i32 0");
        fx.start("uncaught");
        fx.report_pending(&message, "kr_uncaught", &[]);
        fx.runtime("void", "kr_runtime_exit", "");
        fx.line("ret i32 1");
        format!(
            "define i32 @main(i32 %argc, ptr %argv) {{\nallocas:\n{}  br label %entry\n{}}}\n",
            fx.allocas, fx.out
        )
    }

    fn emit_test_main(&mut self, tests: &[(usize, String)]) -> String {
        let inits = self.static_inits();
        let message = self.fn_name(self.message_fn.expect("the uncaught handler is prepared"));
        let cases: Vec<(String, String)> = tests
            .iter()
            .map(|(id, name)| (self.fn_name(*id), self.c_string(name)))
            .collect();
        let mut fx = FnCx::new(self, Subst::new(), Ty::INT);
        fx.start("entry");
        fx.runtime("void", "kr_runtime_init", "i32 %argc, ptr %argv");
        fx.run_static_inits(&inits, "uncaught");
        fx.runtime("void", "kr_test_begin", &format!("i64 {}", cases.len()));
        for (index, (function, name)) in cases.iter().enumerate() {
            fx.line(format!("call void {function}()"));
            let pending = fx.assign("load ptr, ptr @kr_pending");
            let failed = fx.assign(format!("icmp ne ptr {pending}, null"));
            let fail = format!("testfail{index}");
            let pass = format!("testpass{index}");
            let next = format!("testnext{index}");
            fx.line(format!("br i1 {failed}, label %{fail}, label %{pass}"));
            fx.start(&fail);
            fx.report_pending(&message, "kr_test_failed", &[format!("ptr {name}")]);
            fx.line(format!("br label %{next}"));
            fx.start(&pass);
            fx.runtime("void", "kr_test_passed", &format!("ptr {name}"));
            fx.line(format!("br label %{next}"));
            fx.start(&next);
        }
        let code = fx.runtime("i32", "kr_test_end", "");
        fx.runtime("void", "kr_runtime_exit", "");
        fx.line(format!("ret i32 {code}"));
        fx.start("uncaught");
        fx.report_pending(&message, "kr_uncaught", &[]);
        fx.runtime("void", "kr_runtime_exit", "");
        fx.line("ret i32 1");
        format!(
            "define i32 @main(i32 %argc, ptr %argv) {{\nallocas:\n{}  br label %entry\n{}}}\n",
            fx.allocas, fx.out
        )
    }

    fn emit_library(&mut self, prefix: &str, exports: &[(usize, String)]) -> String {
        let inits = self.static_inits();
        let message = self.fn_name(self.message_fn.expect("the uncaught handler is prepared"));
        let once = format!("@\"{prefix}.once\"");
        self.globals
            .push(format!("{once} = internal global i32 0, align 4"));
        let init_name = format!("@{prefix}_init");
        let setup = format!("@\"{prefix}.setup\"");
        let mut out = String::new();
        let mut fx = FnCx::new(self, Subst::new(), Ty::Void);
        fx.start("entry");
        fx.runtime("void", "kr_runtime_init", "i32 0, ptr null");
        fx.run_static_inits(&inits, "uncaught");
        fx.line("ret void");
        fx.start("uncaught");
        fx.report_pending(&message, "kr_export_failed", &[]);
        fx.line("ret void");
        out.push_str(&format!(
            "define internal void {setup}() {{\nallocas:\n{}  br label %entry\n{}}}\n",
            fx.allocas, fx.out
        ));
        let mut fx = FnCx::new(self, Subst::new(), Ty::Void);
        fx.start("entry");
        fx.runtime("void", "kr_once", &format!("ptr {once}, ptr {setup}"));
        fx.line("ret void");
        out.push_str(&format!(
            "define void {init_name}() {{\nallocas:\n{}  br label %entry\n{}}}\n",
            fx.allocas, fx.out
        ));
        for (id, symbol) in exports {
            let instance = self.instances[*id].clone();
            let params = self.instance_params(&instance);
            let ret = self.instance_ret(&instance);
            let function = self.fn_name(*id);
            let mut fx = FnCx::new(self, Subst::new(), ret.clone());
            fx.start("entry");
            fx.line(format!("call void {init_name}()"));
            let mut rendered = Vec::new();
            let mut args = Vec::new();
            for (index, param) in params.iter().enumerate() {
                let ll = fx.ll(param);
                rendered.push(format!("{ll} %a{index}"));
                args.push((ll, format!("%a{index}")));
            }
            let result = fx.call_instance(*id, &args, &ret);
            fx.check_pending("failed");
            let llret = fx.ll(&ret);
            if llret == "void" {
                fx.line("ret void");
            } else {
                fx.line(format!("ret {llret} {result}"));
            }
            fx.start("failed");
            fx.report_pending(&message, "kr_export_failed", &[]);
            fx.ret_zero(&ret);
            let _ = function;
            out.push_str(&format!(
                "define {llret} @{symbol}({}) {{\nallocas:\n{}  br label %entry\n{}}}\n",
                rendered.join(", "),
                fx.allocas,
                fx.out
            ));
        }
        out
    }

    fn c_string(&mut self, text: &str) -> String {
        let name = format!("@\"cstr.{}\"", self.globals.len());
        self.globals.push(format!(
            "{name} = private unnamed_addr constant [{} x i8] c\"{}\\00\"",
            text.len() + 1,
            crate::rtti::escape_bytes(text)
        ));
        name
    }
}

impl FnCx<'_, '_, '_> {
    fn run_static_inits(&mut self, inits: &[(String, Ty, String)], unwind: &str) {
        for (init, ty, global) in inits {
            let ll = self.ll(ty);
            let value = self.assign(format!("call {ll} {init}()"));
            self.check_pending(unwind);
            self.line(format!("store {ll} {value}, ptr {global}"));
        }
    }

    fn report_pending(&mut self, message: &str, reporter: &'static str, extra: &[String]) {
        let throwable = self.cx.lang_ty(LangItem::Throwable);
        let string = self.cx.tcx.string_ty();
        let exception = self.assign("load ptr, ptr @kr_pending");
        self.line("store ptr null, ptr @kr_pending");
        let slot = self.alloca_ll("ptr");
        self.line(format!("store ptr {exception}, ptr {slot}"));
        let text = self.assign(format!("call ptr {message}(ptr {slot})"));
        self.line(format!("store ptr null, ptr {slot}"));
        let descriptor = self.assign(format!("load ptr, ptr {exception}"));
        let mut args: Vec<String> = extra.to_vec();
        args.push(format!("ptr {descriptor}"));
        args.push(format!("ptr {text}"));
        self.runtime("void", reporter, &args.join(", "));
        self.drop_value(&text, &string);
        self.drop_value(&exception, &throwable);
    }
}

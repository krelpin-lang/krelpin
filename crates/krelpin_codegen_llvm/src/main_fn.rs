use krelpin_typeck::{Subst, Ty};

use crate::cx::Cx;
use crate::func::FnCx;
use crate::rtti::RtKey;

impl Cx<'_, '_> {
    pub(crate) fn prepare_main(&mut self) {
        self.message_fn = Some(self.message_instance());
        if let Some((_, true)) = self.entry {
            let array = Ty::Array(Box::new(self.tcx.string_ty()));
            self.rt_key(RtKey::Array(array.clone()));
            self.drop_array_fn(&array);
        }
        let throwable = self.lang_ty(krelpin_typeck::LangItem::Throwable);
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

    pub(crate) fn emit_main(&mut self) -> String {
        let (entry, takes_args) = self.entry.expect("an entry point is compiled");
        let entry_name = self.fn_name(entry);
        let message = self.message_fn.expect("the uncaught handler is prepared");
        let message_name = self.fn_name(message);
        let order = self.static_order();
        let inits: Vec<(String, Ty, String)> = order
            .iter()
            .filter_map(|&id| {
                let info = &self.statics[id];
                let init = info.init?;
                Some((self.fn_name(init), info.ty.clone(), info.global.clone()))
            })
            .collect();
        let args_descriptor = if takes_args {
            let array = Ty::Array(Box::new(self.tcx.string_ty()));
            Some(self.rt_global(RtKey::Array(array)))
        } else {
            None
        };
        let throwable = self.lang_ty(krelpin_typeck::LangItem::Throwable);
        let string = self.tcx.string_ty();
        let mut fx = FnCx::new(self, Subst::new(), Ty::INT);
        fx.start("entry");
        fx.runtime("void", "kr_runtime_init", "i32 %argc, ptr %argv");
        for (init, ty, global) in inits {
            let ll = fx.ll(&ty);
            let value = fx.assign(format!("call {ll} {init}()"));
            fx.check_pending("uncaught");
            fx.line(format!("store {ll} {value}, ptr {global}"));
        }
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
        let exception = fx.assign("load ptr, ptr @kr_pending");
        fx.line("store ptr null, ptr @kr_pending");
        let slot = fx.alloca_ll("ptr");
        fx.line(format!("store ptr {exception}, ptr {slot}"));
        let text = fx.assign(format!("call ptr {message_name}(ptr {slot})"));
        let descriptor = fx.assign(format!("load ptr, ptr {exception}"));
        fx.runtime(
            "void",
            "kr_uncaught",
            &format!("ptr {descriptor}, ptr {text}"),
        );
        fx.drop_value(&text, &string);
        fx.drop_value(&exception, &throwable);
        fx.runtime("void", "kr_runtime_exit", "");
        fx.line("ret i32 1");
        format!(
            "define i32 @main(i32 %argc, ptr %argv) {{\nallocas:\n{}  br label %entry\n{}}}\n",
            fx.allocas, fx.out
        )
    }
}

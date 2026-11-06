//! The futures of `async` methods.
//!
//! Calling an `async` method allocates a frame object, which implements
//! `Future<T>`, stores the arguments in it and returns it. Every local of
//! the method lives in the frame, so a value survives between polls and a
//! reference to a local stays valid: the frame never moves.
//!
//! ```text
//! frame = { descriptor, i32 state, local 1, local 2, ... }
//! ```
//!
//! The frame's `poll` runs the method's MIR body. `state` says where to
//! continue: 0 at the start, `k` after the `k`-th `suspend`, and -1 once the
//! body has returned or thrown. The return value and the `Context` are
//! ordinary stack slots, because they belong to one poll.
//!
//! Locals that are moved out or dropped are zeroed, as everywhere else, so
//! dropping an unfinished frame drops exactly the values it still owns.

use krelpin_mir::BodyId;
use krelpin_typeck::{LangItem, Ty};

use crate::cx::Cx;
use crate::func::FnCx;
use crate::rtti::RtKey;

/// The future of one instance of an `async` body.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct AsyncFrame {
    pub body: BodyId,
    pub args: Vec<Ty>,
}

impl Cx<'_, '_> {
    pub(crate) fn async_frame(&mut self, body: BodyId, args: &[Ty]) -> usize {
        let frame = AsyncFrame {
            body,
            args: args.to_vec(),
        };
        if let Some(&index) = self.async_frame_ids.get(&frame) {
            return index;
        }
        let index = self.async_frames.len();
        self.async_frames.push(frame.clone());
        self.async_frame_ids.insert(frame, index);
        index
    }

    /// The frame's locals, with the method's type arguments filled in.
    pub(crate) fn frame_locals(&self, index: usize) -> Vec<Ty> {
        let frame = &self.async_frames[index];
        let subst = self.body_subst(frame.body, &frame.args);
        self.mir
            .body(frame.body)
            .locals
            .iter()
            .map(|decl| decl.ty.subst(&subst))
            .collect()
    }

    /// What the frame's future produces.
    pub(crate) fn frame_output(&self, index: usize) -> Ty {
        let frame = &self.async_frames[index];
        let subst = self.body_subst(frame.body, &frame.args);
        let asyncness = self
            .mir
            .body(frame.body)
            .asyncness
            .as_ref()
            .expect("frames belong to `async` bodies");
        asyncness.output.subst(&subst)
    }

    pub(crate) fn frame_future(&self, index: usize) -> Ty {
        self.tcx.future_of(self.frame_output(index))
    }

    /// The field that holds local `local`: locals start after the
    /// descriptor and the state.
    pub(crate) fn frame_field(local: usize) -> usize {
        local + 1
    }

    pub(crate) fn frame_type(&mut self, index: usize) -> String {
        if let Some(name) = self.frame_types.get(&index) {
            return name.clone();
        }
        let name = format!("%\"frame.{index}\"");
        self.frame_types.insert(index, name.clone());
        let mut fields = vec!["ptr".to_owned(), "i32".to_owned()];
        for ty in self.frame_locals(index).iter().skip(1) {
            fields.push(self.mem_llty(ty));
        }
        self.type_defs
            .push(format!("{name} = type {{ {} }}", fields.join(", ")));
        name
    }

    pub(crate) fn frame_size(&mut self, index: usize) -> u64 {
        let mut layout = vec![(8, 8), (4, 4)];
        for ty in self.frame_locals(index).iter().skip(1) {
            layout.push(self.size_align(ty));
        }
        crate::layout::struct_layout(&layout).0
    }

    pub(crate) fn frame_name(&self, index: usize) -> String {
        let body = self.mir.body(self.async_frames[index].body);
        format!(
            "{}.{}$future",
            self.tcx.sig(body.owner).name.name,
            body.name
        )
    }

    /// Drops what an unfinished future still owns, then frees it.
    pub(crate) fn drop_frame_fn(&mut self, index: usize) -> String {
        let name = format!("@\"dropframe.{index}\"");
        if !self.frame_drops.insert(index) {
            return name;
        }
        let frame_ty = self.frame_type(index);
        let locals = self.frame_locals(index);
        let mut fx = FnCx::new(self, Default::default(), Ty::Void);
        fx.start("entry");
        for (local, ty) in locals.iter().enumerate().skip(1) {
            if *ty == Ty::Void || !fx.cx.needs_drop(ty) {
                continue;
            }
            let address = fx.assign(format!(
                "getelementptr inbounds {frame_ty}, ptr %v, i32 0, i32 {}",
                Cx::frame_field(local)
            ));
            fx.drop_at(ty, &address);
        }
        fx.runtime("void", "kr_free", "ptr %v");
        fx.line("ret void");
        let body = fx.out;
        let allocas = fx.allocas;
        self.functions.push(crate::func::define(
            "void", &name, "ptr %v", &allocas, &body,
        ));
        name
    }

    /// The function an `async` method compiles to: it puts the arguments in
    /// a new frame and returns the frame as a `Future`.
    pub(crate) fn emit_async_start(&mut self, id: usize, body: BodyId, args: &[Ty]) -> String {
        let index = self.async_frame(body, args);
        let frame_ty = self.frame_type(index);
        let descriptor = self.rt_global(RtKey::Async(index));
        let locals = self.frame_locals(index);
        let arg_count = self.mir.body(body).arg_count;
        let future = self.frame_future(index);
        let name = self.fn_name(id);
        let subst = self.body_subst(body, args);
        let mut fx = FnCx::new(self, subst, future);
        fx.start("entry");
        let frame = fx.runtime("ptr", "kr_new_object", &format!("ptr {descriptor}"));
        let mut params = Vec::new();
        for (local, ty) in locals.iter().enumerate().take(arg_count + 1).skip(1) {
            let ll = fx.ll(ty);
            params.push(format!("{ll} %a{local}"));
            if *ty == Ty::Void {
                continue;
            }
            let field = fx.assign(format!(
                "getelementptr inbounds {frame_ty}, ptr {frame}, i32 0, i32 {}",
                Cx::frame_field(local)
            ));
            fx.line(format!("store {ll} %a{local}, ptr {field}"));
        }
        fx.line(format!("ret ptr {frame}"));
        fx.into_function(&name, &params, "ptr")
    }

    pub(crate) fn is_future_poll(&self, selector: &crate::instance::Selector) -> bool {
        self.tcx.is_lang_ty(&selector.owner, LangItem::Future) && selector.name.as_str() == "poll"
    }
}

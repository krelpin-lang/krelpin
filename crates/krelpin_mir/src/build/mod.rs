mod expr;
mod matches;
mod stmt;

use std::collections::{HashMap, HashSet};

use krelpin_hir::{self as hir, BodyKind, BodyValue, ClosureBody, LocalId};
use krelpin_span::def_id::DefId;
use krelpin_span::{Span, Symbol};
use krelpin_typeck::{AdtKind, Tcx, Ty};

use crate::mir::{
    BasicBlock, BasicBlockData, Body, BodyId, BodySource, CaptureInfo, Local, LocalDecl, Mir,
    Place, PlaceElem, Rvalue, Statement, StatementKind, Terminator, TerminatorKind, Unwind,
};
use crate::visit::for_each_local_mut;

pub fn build(hir: &hir::Hir, tcx: &Tcx<'_>) -> Mir {
    let mut cx = MirCtxt {
        tcx,
        constructed: hir
            .bodies
            .iter()
            .filter(|body| body.kind == BodyKind::Constructor)
            .map(|body| body.owner)
            .collect(),
        bodies: Vec::new(),
    };
    for body in &hir.bodies {
        Builder::build_item(&mut cx, body);
    }
    Mir {
        bodies: cx
            .bodies
            .into_iter()
            .map(|body| body.expect("every reserved body is built"))
            .collect(),
    }
}

struct MirCtxt<'a, 'tcx> {
    tcx: &'a Tcx<'tcx>,
    constructed: HashSet<DefId>,
    bodies: Vec<Option<Body>>,
}

impl MirCtxt<'_, '_> {
    fn reserve(&mut self) -> BodyId {
        let id = BodyId(self.bodies.len() as u32);
        self.bodies.push(None);
        id
    }
}

enum Scope<'h> {
    Block {
        locals: Vec<Local>,
    },
    Loop {
        exit: BasicBlock,
        next: BasicBlock,
    },
    Breakable {
        exit: BasicBlock,
    },
    Try {
        finally: Option<&'h hir::Block>,
        handler: Option<BasicBlock>,
    },
    Value {
        dest: Place,
        exit: BasicBlock,
    },
}

struct BlockBuilder {
    statements: Vec<Statement>,
    terminator: Option<Terminator>,
}

struct Builder<'a, 'h, 'tcx> {
    cx: &'a mut MirCtxt<'h, 'tcx>,
    hir: &'h hir::Body,
    id: BodyId,
    ret: Ty,
    locals: Vec<LocalDecl>,
    blocks: Vec<BlockBuilder>,
    current: BasicBlock,
    vars: HashMap<LocalId, Place>,
    scopes: Vec<Scope<'h>>,
    closures: usize,
}

impl<'a, 'h, 'tcx> Builder<'a, 'h, 'tcx> {
    fn new(cx: &'a mut MirCtxt<'h, 'tcx>, hir: &'h hir::Body, ret: Ty) -> Self {
        let id = cx.reserve();
        let mut builder = Builder {
            cx,
            hir,
            id,
            ret: ret.clone(),
            locals: Vec::new(),
            blocks: Vec::new(),
            current: BasicBlock::START,
            vars: HashMap::new(),
            scopes: Vec::new(),
            closures: 0,
        };
        builder.push_local(None, ret, hir.span);
        builder.current = builder.new_block();
        builder
    }

    fn build_item(cx: &'a mut MirCtxt<'h, 'tcx>, body: &'h hir::Body) {
        let mut builder = Builder::new(cx, body, body.ret.clone());
        for &local in body.this.iter().chain(&body.params) {
            builder.declare_arg(local);
        }
        let arg_count = builder.locals.len() - 1;
        match &body.value {
            BodyValue::Block(block) => builder.lower_block(block),
            BodyValue::Expr(expr) => {
                builder.expr_into(Place::local(Local::RETURN), expr);
                builder.terminate(TerminatorKind::Return, expr.span);
            }
        }
        let source = match body.kind {
            BodyKind::Method => BodySource::Method,
            BodyKind::Constructor => BodySource::Constructor,
            BodyKind::FieldInit => BodySource::FieldInit,
        };
        builder.finish(source, arg_count, Vec::new());
    }

    fn build_closure(
        &mut self,
        closure: &'h hir::Closure,
        params: &[LocalId],
        ret: &Ty,
        body: &'h ClosureBody,
        ty: &Ty,
        span: Span,
    ) -> BodyId {
        let parent = self.id;
        let mut builder = Builder::new(self.cx, self.hir, ret.clone());
        let env = builder.push_local(None, ty.clone(), span);
        let mut captures = Vec::new();
        for (index, capture) in closure.captures.iter().enumerate() {
            let decl = self.hir.local(capture.local);
            captures.push(CaptureInfo {
                name: decl.name,
                ty: decl.ty.clone(),
                mutable: capture.mutable,
            });
            let place = Place::local(env)
                .project(PlaceElem::Capture(index))
                .project(PlaceElem::Deref);
            builder.vars.insert(capture.local, place);
        }
        for &param in params {
            builder.declare_arg(param);
        }
        let arg_count = builder.locals.len() - 1;
        match body {
            ClosureBody::Expr(expr) => {
                builder.expr_into(Place::local(Local::RETURN), expr);
                builder.terminate(TerminatorKind::Return, expr.span);
            }
            ClosureBody::Block(block) => builder.lower_block(block),
        }
        let source = BodySource::Closure {
            parent,
            index: self.closures,
        };
        self.closures += 1;
        builder.finish(source, arg_count, captures)
    }

    fn finish(
        mut self,
        source: BodySource,
        arg_count: usize,
        captures: Vec<CaptureInfo>,
    ) -> BodyId {
        let fallthrough = if self.ret == Ty::Void {
            TerminatorKind::Return
        } else {
            TerminatorKind::Unreachable
        };
        self.terminate(fallthrough, self.hir.span);
        let mut blocks = simplify(self.blocks);
        let locals = remove_unused_locals(self.locals, arg_count, &mut blocks);
        let body = Body {
            owner: self.hir.owner,
            name: self.hir.name,
            source,
            locals,
            arg_count,
            captures,
            blocks,
            span: self.hir.span,
        };
        self.cx.bodies[self.id.index()] = Some(body);
        self.id
    }

    fn tcx(&self) -> &'a Tcx<'tcx> {
        self.cx.tcx
    }

    fn push_local(&mut self, name: Option<Symbol>, ty: Ty, span: Span) -> Local {
        let local = Local(self.locals.len() as u32);
        self.locals.push(LocalDecl { name, ty, span });
        local
    }

    fn temp(&mut self, ty: &Ty, span: Span) -> Local {
        self.push_local(None, ty.clone(), span)
    }

    fn declare_arg(&mut self, id: LocalId) -> Local {
        let decl = self.hir.local(id);
        let local = self.push_local(Some(decl.name), decl.ty.clone(), decl.span);
        self.vars.insert(id, Place::local(local));
        local
    }

    fn declare_var(&mut self, id: LocalId) -> Local {
        let decl = self.hir.local(id);
        let local = self.push_local(Some(decl.name), decl.ty.clone(), decl.span);
        self.vars.insert(id, Place::local(local));
        self.push(StatementKind::StorageLive(local), decl.span);
        let scope = self.scopes.iter_mut().rev().find_map(|scope| match scope {
            Scope::Block { locals } => Some(locals),
            _ => None,
        });
        if let Some(locals) = scope {
            locals.push(local);
        }
        local
    }

    fn var(&self, id: LocalId) -> Place {
        self.vars
            .get(&id)
            .cloned()
            .unwrap_or_else(|| unreachable!("local {id:?} is used before it is declared"))
    }

    fn new_block(&mut self) -> BasicBlock {
        let block = BasicBlock(self.blocks.len() as u32);
        self.blocks.push(BlockBuilder {
            statements: Vec::new(),
            terminator: None,
        });
        block
    }

    fn push(&mut self, kind: StatementKind, span: Span) {
        let block = &mut self.blocks[self.current.index()];
        if block.terminator.is_none() {
            block.statements.push(Statement { kind, span });
        }
    }

    fn assign(&mut self, place: Place, rvalue: Rvalue, span: Span) {
        self.push(StatementKind::Assign(Box::new((place, rvalue))), span);
    }

    fn terminate(&mut self, kind: TerminatorKind, span: Span) {
        let block = &mut self.blocks[self.current.index()];
        if block.terminator.is_none() {
            block.terminator = Some(Terminator { kind, span });
        }
    }

    fn goto(&mut self, target: BasicBlock, span: Span) {
        self.terminate(TerminatorKind::Goto { target }, span);
    }

    fn goto_new(&mut self, span: Span) -> BasicBlock {
        let next = self.new_block();
        self.goto(next, span);
        self.current = next;
        next
    }

    fn diverge(&mut self, kind: TerminatorKind, span: Span) {
        self.terminate(kind, span);
        self.current = self.new_block();
    }

    fn unwind(&self) -> Unwind {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| match scope {
                Scope::Try {
                    handler: Some(handler),
                    ..
                } => Some(Unwind::Handler(*handler)),
                _ => None,
            })
            .unwrap_or(Unwind::Resume)
    }

    fn push_block_scope(&mut self) {
        self.scopes.push(Scope::Block { locals: Vec::new() });
    }

    fn pop_block_scope(&mut self, span: Span) {
        if let Some(Scope::Block { locals }) = self.scopes.pop() {
            for local in locals.into_iter().rev() {
                self.push(StatementKind::StorageDead(local), span);
            }
        }
    }

    fn exit_scopes(&mut self, depth: usize, span: Span) {
        let mut index = self.scopes.len();
        while index > depth {
            index -= 1;
            match &self.scopes[index] {
                Scope::Block { locals } => {
                    for local in locals.clone().into_iter().rev() {
                        self.push(StatementKind::StorageDead(local), span);
                    }
                }
                Scope::Try {
                    finally: Some(finally),
                    ..
                } => {
                    let finally = *finally;
                    let saved = self.scopes.split_off(index);
                    self.lower_block(finally);
                    self.scopes.extend(saved);
                }
                Scope::Loop { .. }
                | Scope::Breakable { .. }
                | Scope::Try { finally: None, .. }
                | Scope::Value { .. } => {}
            }
        }
    }

    fn is_copy(&self, ty: &Ty) -> bool {
        self.is_copy_inner(ty, &mut Vec::new())
    }

    fn is_copy_inner(&self, ty: &Ty, visiting: &mut Vec<DefId>) -> bool {
        match ty {
            Ty::Primitive(_) | Ty::Null | Ty::Void | Ty::Unknown => true,
            Ty::Ref { mutable, .. } => !mutable,
            Ty::Nullable(inner) => self.is_copy_inner(inner, visiting),
            Ty::Array(_) | Ty::Param(_) | Ty::External { .. } => false,
            Ty::Adt { def, args, .. } => {
                if visiting.contains(def) {
                    return false;
                }
                let sig = self.tcx().sig(*def);
                match sig.kind {
                    AdtKind::Enum => sig.variants.iter().all(|variant| variant.fields.is_empty()),
                    AdtKind::Record => {
                        visiting.push(*def);
                        let subst = self.tcx().subst_for(*def, args);
                        let copy = sig.components.iter().all(|name| {
                            sig.fields.get(name).is_some_and(|field| {
                                self.is_copy_inner(&field.ty.subst(&subst), visiting)
                            })
                        });
                        visiting.pop();
                        copy
                    }
                    AdtKind::Class | AdtKind::Interface => false,
                }
            }
        }
    }
}

fn simplify(blocks: Vec<BlockBuilder>) -> Vec<BasicBlockData> {
    let mut blocks: Vec<BasicBlockData> = blocks
        .into_iter()
        .map(|block| BasicBlockData {
            statements: block.statements,
            terminator: block.terminator.unwrap_or(Terminator {
                kind: TerminatorKind::Unreachable,
                span: Span::DUMMY,
            }),
        })
        .collect();

    let forwards: Vec<BasicBlock> = (0..blocks.len())
        .map(|index| skip_empty_gotos(&blocks, BasicBlock(index as u32)))
        .collect();
    for block in &mut blocks {
        for target in block.terminator.kind.successors_mut() {
            *target = forwards[target.index()];
        }
    }

    let mut reachable = vec![false; blocks.len()];
    let mut stack = vec![BasicBlock::START];
    while let Some(block) = stack.pop() {
        if std::mem::replace(&mut reachable[block.index()], true) {
            continue;
        }
        stack.extend(blocks[block.index()].terminator.kind.successors());
    }

    let mut renumbered = vec![None; blocks.len()];
    let mut next = 0;
    for (index, live) in reachable.iter().enumerate() {
        if *live {
            renumbered[index] = Some(BasicBlock(next));
            next += 1;
        }
    }
    let mut kept = Vec::with_capacity(next as usize);
    for (index, mut block) in blocks.drain(..).enumerate() {
        if !reachable[index] {
            continue;
        }
        for target in block.terminator.kind.successors_mut() {
            *target = renumbered[target.index()].expect("successors of live blocks are live");
        }
        kept.push(block);
    }
    kept
}

fn skip_empty_gotos(blocks: &[BasicBlockData], mut block: BasicBlock) -> BasicBlock {
    for _ in 0..blocks.len() {
        let data = &blocks[block.index()];
        match data.terminator.kind {
            TerminatorKind::Goto { target } if data.statements.is_empty() && target != block => {
                block = target;
            }
            _ => break,
        }
    }
    block
}

fn remove_unused_locals(
    locals: Vec<LocalDecl>,
    arg_count: usize,
    blocks: &mut [BasicBlockData],
) -> Vec<LocalDecl> {
    let mut used = vec![false; locals.len()];
    for flag in used.iter_mut().take(arg_count + 1) {
        *flag = true;
    }
    for_each_local_mut(blocks, &mut |local| used[local.index()] = true);

    let mut renumbered = vec![None; locals.len()];
    let mut kept = Vec::with_capacity(locals.len());
    for (index, decl) in locals.into_iter().enumerate() {
        if used[index] {
            renumbered[index] = Some(Local(kept.len() as u32));
            kept.push(decl);
        }
    }
    for_each_local_mut(blocks, &mut |local| {
        *local = renumbered[local.index()].expect("every mentioned local is kept");
    });
    kept
}

use krelpin_span::{Span, Symbol, kw};

use crate::token::Lit;

mod display;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Ident {
    pub name: Symbol,
    pub span: Span,
}

impl Ident {
    pub fn new(name: Symbol, span: Span) -> Ident {
        Ident { name, span }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Path {
    pub segments: Vec<Ident>,
    pub span: Span,
}

impl Path {
    pub fn new(segments: Vec<Ident>) -> Path {
        let span = match (segments.first(), segments.last()) {
            (Some(first), Some(last)) => first.span.to(last.span),
            _ => Span::DUMMY,
        };
        Path { segments, span }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompilationUnit {
    pub docs: Vec<Symbol>,
    pub package: Option<Path>,
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    pub path: Path,
    pub glob: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    pub path: Path,
    pub args: Option<Unparsed>,
    pub span: Span,
}

impl Annotation {
    pub fn is(&self, name: &str) -> bool {
        matches!(self.path.segments.as_slice(), [segment] if segment.name.as_str() == name)
    }
}

pub fn is_intrinsic(annotations: &[Annotation]) -> bool {
    annotations
        .iter()
        .any(|annotation| annotation.is("Intrinsic"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Default,
    Public(Span),
    Protected(Span),
    Private(Span),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldStorage {
    Instance,
    Const(Span),
    Shared(Span),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub docs: Vec<Symbol>,
    pub annotations: Vec<Annotation>,
    pub vis: Visibility,
    pub inheritance: Inheritance,
    pub name: Ident,
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Class(Class),
    Struct(Struct),
    Interface(Interface),
    Enum(Enum),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inheritance {
    pub abstract_kw: Option<Span>,
    pub final_kw: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Class {
    pub generics: Generics,
    pub extends: Option<Ty>,
    pub implements: Vec<Ty>,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Struct {
    pub generics: Generics,
    pub implements: Vec<Ty>,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub generics: Generics,
    pub extends: Vec<Ty>,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enum {
    pub implements: Vec<Ty>,
    pub constants: Vec<EnumConstant>,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumConstant {
    pub docs: Vec<Symbol>,
    pub annotations: Vec<Annotation>,
    pub name: Ident,
    pub args: Vec<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub docs: Vec<Symbol>,
    pub annotations: Vec<Annotation>,
    pub vis: Visibility,
    pub inheritance: Inheritance,
    pub kind: MemberKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Field(Field),
    Constructor(Constructor),
    Method(Method),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub storage: FieldStorage,
    pub ty: Ty,
    pub name: Ident,
    pub init: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Constructor {
    pub name: Ident,
    pub params: Vec<Param>,
    pub throws: Vec<Ty>,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Method {
    pub generics: Generics,
    pub ret: FnRetTy,
    pub name: Ident,
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    pub throws: Vec<Ty>,
    pub body: Option<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FnRetTy {
    Void(Span),
    Ty(Ty),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receiver {
    pub kind: ReceiverKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiverKind {
    Owned,
    Ref,
    RefMut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub ty: Ty,
    pub name: Ident,
    pub variadic: bool,
    pub span: Span,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Generics {
    pub params: Vec<GenericParam>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericParam {
    pub name: Ident,
    pub kind: GenericParamKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenericParamKind {
    Type { bounds: Vec<Ty> },
    BorrowSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ty {
    pub kind: TyKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TyKind {
    Primitive(PrimitiveTy),
    Path {
        path: Path,
        args: Vec<Ty>,
    },
    Ref {
        mutable: bool,
        sources: Vec<Ident>,
        ty: Box<Ty>,
    },
    Nullable(Box<Ty>),
    Array(Box<Ty>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveTy {
    Boolean,
    Byte,
    Short,
    Int,
    Long,
    Char,
    Float,
    Double,
}

impl PrimitiveTy {
    pub fn from_symbol(symbol: Symbol) -> Option<PrimitiveTy> {
        let primitive = match symbol {
            kw::Boolean => PrimitiveTy::Boolean,
            kw::Byte => PrimitiveTy::Byte,
            kw::Short => PrimitiveTy::Short,
            kw::Int => PrimitiveTy::Int,
            kw::Long => PrimitiveTy::Long,
            kw::Char => PrimitiveTy::Char,
            kw::Float => PrimitiveTy::Float,
            kw::Double => PrimitiveTy::Double,
            _ => return None,
        };
        Some(primitive)
    }

    pub fn name(self) -> &'static str {
        match self {
            PrimitiveTy::Boolean => "boolean",
            PrimitiveTy::Byte => "byte",
            PrimitiveTy::Short => "short",
            PrimitiveTy::Int => "int",
            PrimitiveTy::Long => "long",
            PrimitiveTy::Char => "char",
            PrimitiveTy::Float => "float",
            PrimitiveTy::Double => "double",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unparsed {
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StmtKind {
    Local(Local),
    Expr(Expr),
    Block(Block),
    If {
        cond: Expr,
        then: Box<Stmt>,
        els: Option<Box<Stmt>>,
    },
    While {
        cond: Expr,
        body: Box<Stmt>,
    },
    DoWhile {
        body: Box<Stmt>,
        cond: Expr,
    },
    For {
        init: Vec<Stmt>,
        cond: Option<Expr>,
        update: Vec<Expr>,
        body: Box<Stmt>,
    },
    ForEach {
        ty: Ty,
        name: Ident,
        iter: Expr,
        body: Box<Stmt>,
    },
    Switch {
        scrutinee: Expr,
        arms: Vec<SwitchArm>,
    },
    Try {
        body: Block,
        catches: Vec<Catch>,
        finally: Option<Block>,
    },
    Return(Option<Expr>),
    Yield(Expr),
    Throw(Expr),
    Break,
    Continue,
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub ty: Ty,
    pub name: Ident,
    pub init: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchArm {
    pub labels: Option<Vec<Expr>>,
    pub body: Box<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catch {
    pub tys: Vec<Ty>,
    pub name: Ident,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprKind {
    Lit(Lit),
    Bool(bool),
    Null,
    This,
    Super,
    Name(Ident),
    Field {
        expr: Box<Expr>,
        name: Ident,
        safe: bool,
    },
    MethodCall {
        receiver: Option<Box<Expr>>,
        name: Ident,
        args: Vec<Expr>,
        safe: bool,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Index {
        expr: Box<Expr>,
        index: Box<Expr>,
    },
    New {
        ty: Ty,
        args: Vec<Expr>,
    },
    NewArray {
        elem: Ty,
        len: Box<Expr>,
    },
    ArrayInit {
        ty: Option<Ty>,
        elems: Vec<Expr>,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Borrow {
        mutable: bool,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Assign {
        op: Option<BinOp>,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
    InstanceOf {
        expr: Box<Expr>,
        ty: Ty,
        binding: Option<Ident>,
    },
    Cast {
        ty: Ty,
        expr: Box<Expr>,
    },
    Lambda {
        params: Vec<LambdaParam>,
        body: Box<LambdaBody>,
    },
    MethodRef {
        expr: Box<Expr>,
        name: Ident,
    },
    Switch {
        scrutinee: Box<Expr>,
        arms: Vec<SwitchExprArm>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchExprArm {
    pub labels: Option<Vec<Expr>>,
    pub body: SwitchExprBody,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwitchExprBody {
    Expr(Expr),
    Block(Block),
    Throw(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
    PreInc,
    PreDec,
    PostInc,
    PostDec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    BitOr,
    BitXor,
    BitAnd,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Elvis,
    Shl,
    Shr,
    UShr,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Or => "||",
            BinOp::And => "&&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::BitAnd => "&",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Elvis => "?:",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::UShr => ">>>",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LambdaParam {
    pub ty: Option<Ty>,
    pub name: Ident,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LambdaBody {
    Expr(Expr),
    Block(Block),
}

use std::fmt;

use krelpin_span::{Span, Symbol, kw};

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
    pub name: Ident,
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Class(Class),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Class {
    pub generics: Generics,
    pub extends: Option<Ty>,
    pub implements: Vec<Ty>,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub docs: Vec<Symbol>,
    pub annotations: Vec<Annotation>,
    pub vis: Visibility,
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
    pub init: Option<Unparsed>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Constructor {
    pub name: Ident,
    pub params: Vec<Param>,
    pub throws: Vec<Ty>,
    pub body: Unparsed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Method {
    pub generics: Generics,
    pub ret: FnRetTy,
    pub name: Ident,
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    pub throws: Vec<Ty>,
    pub body: Option<Unparsed>,
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

impl fmt::Debug for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} @ {:?}", self.name, self.span)
    }
}

impl fmt::Debug for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{self}\" @ {:?}", self.span)
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.segments.iter().map(|s| s.name.as_str()).collect();
        f.write_str(&names.join("."))
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            TyKind::Primitive(primitive) => f.write_str(primitive.name()),
            TyKind::Path { path, args } => {
                write!(f, "{path}")?;
                if !args.is_empty() {
                    let args: Vec<String> = args.iter().map(Ty::to_string).collect();
                    write!(f, "<{}>", args.join(", "))?;
                }
                Ok(())
            }
            TyKind::Ref {
                mutable,
                sources,
                ty,
            } => {
                f.write_str("&")?;
                if *mutable {
                    f.write_str("mut ")?;
                }
                if !sources.is_empty() {
                    let names: Vec<&str> = sources.iter().map(|s| s.name.as_str()).collect();
                    write!(f, "[{}] ", names.join(", "))?;
                }
                match ty.kind {
                    TyKind::Nullable(_) => write!(f, "({ty})"),
                    _ => write!(f, "{ty}"),
                }
            }
            TyKind::Nullable(ty) => write!(f, "{ty}?"),
        }
    }
}

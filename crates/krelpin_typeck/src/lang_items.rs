use krelpin_ast::ast::PrimitiveTy;
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_resolve::ItemTable;
use krelpin_span::def_id::DefId;

macro_rules! lang_items {
    ($($variant:ident: $path:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum LangItem {
            $($variant,)*
        }

        impl LangItem {
            const ALL: &[LangItem] = &[$(LangItem::$variant,)*];

            pub fn path(self) -> &'static str {
                match self {
                    $(LangItem::$variant => $path,)*
                }
            }
        }
    };
}

lang_items! {
    Object: "std.lang.Object",
    String: "std.lang.String",
    Enum: "std.lang.Enum",
    Iterable: "std.lang.Iterable",
    Throwable: "std.lang.Throwable",
    Exception: "std.lang.Exception",
    RuntimeException: "std.lang.RuntimeException",
    Error: "std.lang.Error",
    Boolean: "std.primitive.Boolean",
    Byte: "std.primitive.Byte",
    Short: "std.primitive.Short",
    Int: "std.primitive.Int",
    Long: "std.primitive.Long",
    Char: "std.primitive.Char",
    Float: "std.primitive.Float",
    Double: "std.primitive.Double",
}

const PRIMITIVES: [PrimitiveTy; 8] = [
    PrimitiveTy::Boolean,
    PrimitiveTy::Byte,
    PrimitiveTy::Short,
    PrimitiveTy::Int,
    PrimitiveTy::Long,
    PrimitiveTy::Char,
    PrimitiveTy::Float,
    PrimitiveTy::Double,
];

impl LangItem {
    pub fn primitive(primitive: PrimitiveTy) -> LangItem {
        match primitive {
            PrimitiveTy::Boolean => LangItem::Boolean,
            PrimitiveTy::Byte => LangItem::Byte,
            PrimitiveTy::Short => LangItem::Short,
            PrimitiveTy::Int => LangItem::Int,
            PrimitiveTy::Long => LangItem::Long,
            PrimitiveTy::Char => LangItem::Char,
            PrimitiveTy::Float => LangItem::Float,
            PrimitiveTy::Double => LangItem::Double,
        }
    }
}

#[derive(Debug)]
pub struct LangItems {
    defs: Vec<Option<DefId>>,
}

impl LangItems {
    pub(crate) fn collect(items: &ItemTable, dcx: &DiagCtxt) -> LangItems {
        let defs = LangItem::ALL
            .iter()
            .map(|&item| {
                let def = items.lookup_path(item.path());
                if def.is_none() {
                    dcx.emit(
                        Diagnostic::error(format!("missing lang item `{}`", item.path()))
                            .with_note("the compiler needs this type from the standard library"),
                    );
                }
                def
            })
            .collect();
        LangItems { defs }
    }

    pub fn get(&self, item: LangItem) -> Option<DefId> {
        self.defs[item as usize]
    }

    pub fn is(&self, def: DefId, item: LangItem) -> bool {
        self.get(item) == Some(def)
    }

    pub fn primitive_impl(&self, primitive: PrimitiveTy) -> Option<DefId> {
        self.get(LangItem::primitive(primitive))
    }

    pub fn primitive_of(&self, def: DefId) -> Option<PrimitiveTy> {
        PRIMITIVES
            .into_iter()
            .find(|&primitive| self.primitive_impl(primitive) == Some(def))
    }
}

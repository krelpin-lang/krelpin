use std::collections::HashSet;

use crate::symbol::keyword_texts;
use crate::{BytePos, SourceMap, Span, Symbol, kw};

#[test]
fn keywords_are_prefilled_in_order() {
    assert_eq!(Symbol::intern("class"), kw::Class);
    assert_eq!(Symbol::intern("this"), kw::This);
    assert_eq!(kw::Mut.as_str(), "mut");
    assert!(kw::Null.is_keyword());
    assert_eq!(Symbol::intern("shared"), kw::Shared);
    assert_eq!(Symbol::intern("new"), kw::New);
}

#[test]
fn keywords_are_unique() {
    let unique: HashSet<_> = keyword_texts().iter().collect();
    assert_eq!(unique.len(), keyword_texts().len());
}

#[test]
fn identifiers_are_not_keywords() {
    let symbol = Symbol::intern("Person");
    assert!(!symbol.is_keyword());
    assert_eq!(symbol.as_str(), "Person");
    assert_eq!(Symbol::intern("Person"), symbol);
    assert_ne!(Symbol::intern("person"), symbol);
}

#[test]
fn line_and_column_lookup() {
    let source_map = SourceMap::new();
    let file = source_map.new_source_file("main.kr", "class A {\n    é x;\n}\n".to_owned());

    let loc = source_map.lookup_char_pos(BytePos(0)).unwrap();
    assert_eq!((loc.line, loc.col), (1, 1));

    let x = BytePos(file.src.find('x').unwrap() as u32);
    let loc = source_map.lookup_char_pos(x).unwrap();
    assert_eq!((loc.line, loc.col), (2, 7));

    assert_eq!(file.line(1), "    é x;");
    assert_eq!(file.line_count(), 4);
}

#[test]
fn files_get_disjoint_positions() {
    let source_map = SourceMap::new();
    let first = source_map.new_source_file("a.kr", "abc".to_owned());
    let second = source_map.new_source_file("b.kr", "xyz".to_owned());

    assert_eq!(first.start_pos, BytePos(0));
    assert_eq!(second.start_pos, BytePos(4));

    let loc = source_map.lookup_char_pos(BytePos(5)).unwrap();
    assert_eq!(loc.file.name.to_str(), Some("b.kr"));
    assert_eq!((loc.line, loc.col), (1, 2));

    let span = Span::new(BytePos(4), BytePos(6));
    assert_eq!(source_map.span_to_snippet(span).as_deref(), Some("xy"));
}

#[test]
fn span_helpers() {
    let a = Span::new(BytePos(2), BytePos(5));
    let b = Span::new(BytePos(8), BytePos(10));
    assert_eq!(a.to(b), Span::new(BytePos(2), BytePos(10)));
    assert_eq!(a.subspan(1..2), Span::new(BytePos(3), BytePos(4)));
    assert_eq!(a.len(), 3);
    assert!(a.shrink_to_hi().is_empty());
}

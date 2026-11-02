use std::collections::HashSet;
use std::thread;

use crate::symbol::keyword_texts;
use crate::{BytePos, Expansion, SourceFile, SourceMap, Span, Symbol, kw};

#[test]
fn keywords_are_prefilled_in_order() {
    assert_eq!(Symbol::intern("class"), kw::Class);
    assert_eq!(Symbol::intern("this"), kw::This);
    assert_eq!(kw::Mut.as_str(), "mut");
    assert!(kw::Null.is_keyword());
    assert_eq!(Symbol::intern("static"), kw::Static);
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

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn shared_types_are_thread_safe() {
    assert_send_sync::<Symbol>();
    assert_send_sync::<SourceMap>();
    assert_send_sync::<SourceFile>();
}

#[test]
fn symbols_are_shared_between_threads() {
    let symbols: Vec<Symbol> = thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| Symbol::intern("sharedAcrossThreads")))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    assert!(symbols.windows(2).all(|pair| pair[0] == pair[1]));

    let from_other_thread = thread::spawn(|| Symbol::intern("internedElsewhere"))
        .join()
        .unwrap();
    assert_eq!(from_other_thread.as_str(), "internedElsewhere");
    assert_eq!(Symbol::intern("internedElsewhere"), from_other_thread);
}

#[test]
fn source_files_can_be_added_from_many_threads() {
    let source_map = SourceMap::new();
    thread::scope(|scope| {
        for index in 0..8 {
            let source_map = &source_map;
            scope.spawn(move || source_map.new_source_file(format!("{index}.kr"), "x".repeat(10)));
        }
    });

    for pos in [0, 11, 22, 33, 44, 55, 66, 77] {
        assert!(source_map.lookup_file(BytePos(pos)).is_some());
    }
}

#[test]
fn expansion_files_point_back_to_their_call_site() {
    let source_map = SourceMap::new();
    let user = source_map.new_source_file("a.kr", "@Derive(Equatable) class A {}".to_owned());
    let call_site = Span::new(user.start_pos, user.start_pos + 18);
    let generated = source_map.new_expansion_file(
        "<@Derive on A>",
        "boolean equals(&this, &A other) {}".to_owned(),
        Expansion {
            call_site,
            macro_name: "Derive".to_owned(),
        },
    );
    let inside = Span::new(generated.start_pos + 8, generated.start_pos + 14);
    let expansion = source_map.expansion_of(inside).expect("an expansion");
    assert_eq!(expansion.call_site, call_site);
    assert!(source_map.expansion_of(call_site).is_none());
}

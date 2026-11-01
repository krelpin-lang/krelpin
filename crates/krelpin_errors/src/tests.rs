use std::sync::Arc;
use std::thread;

use krelpin_span::{BytePos, SourceMap, Span};

use crate::{DiagCtxt, Diagnostic, DiagnosticBuffer, HumanEmitter};

fn render(src: &str, diagnostic: Diagnostic) -> String {
    let source_map = Arc::new(SourceMap::new());
    source_map.new_source_file("main.kr", src.to_owned());
    HumanEmitter::new(source_map, Vec::new(), false).render(&diagnostic)
}

fn span(lo: u32, hi: u32) -> Span {
    Span::new(BytePos(lo), BytePos(hi))
}

#[test]
fn renders_span_with_label_and_children() {
    let src = "class Main {\n    String s = \"abc;\n}\n";
    let lo = src.find('"').unwrap() as u32;
    let diagnostic = Diagnostic::error("unterminated double quote string")
        .with_span(span(lo, lo + 5))
        .with_label("string starts here")
        .with_help("add a closing `\"`");

    let expected = "\
error: unterminated double quote string
 --> main.kr:2:16
  |
2 |     String s = \"abc;
  |                ^^^^^ string starts here
  |
  = help: add a closing `\"`

";
    assert_eq!(render(src, diagnostic), expected);
}

#[test]
fn renders_without_span() {
    let diagnostic = Diagnostic::error("aborting due to 1 previous error");
    assert_eq!(
        render("", diagnostic),
        "error: aborting due to 1 previous error\n\n"
    );
}

#[test]
fn gutter_grows_with_line_number() {
    let src = "\n".repeat(11) + "x";
    let diagnostic = Diagnostic::error("unexpected").with_span(span(11, 12));
    let expected = "\
error: unexpected
  --> main.kr:12:1
   |
12 | x
   | ^

";
    assert_eq!(render(&src, diagnostic), expected);
}

#[test]
fn expands_tabs_and_marks_empty_spans() {
    let src = "\tx";
    let diagnostic = Diagnostic::error("here").with_span(span(2, 2));
    let expected = "\
error: here
 --> main.kr:1:3
  |
1 |     x
  |      ^

";
    assert_eq!(render(src, diagnostic), expected);
}

#[test]
fn context_counts_errors_and_warnings() {
    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());

    dcx.emit(Diagnostic::warning("unused"));
    assert!(!dcx.has_errors());

    dcx.emit(Diagnostic::error("broken"));
    assert_eq!(dcx.error_count(), 1);
    assert_eq!(dcx.warning_count(), 1);
    assert_eq!(buffer.messages(), ["unused", "broken"]);
}

#[test]
fn context_can_be_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DiagCtxt>();

    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());
    thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..100 {
                    dcx.emit(Diagnostic::error("parallel"));
                }
            });
        }
    });

    assert_eq!(dcx.error_count(), 800);
    assert_eq!(buffer.diagnostics().len(), 800);
}

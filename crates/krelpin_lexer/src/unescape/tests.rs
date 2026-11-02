use super::{EscapeError, Mode, strip_indent, unescape};

fn chars(src: &str) -> Result<String, EscapeError> {
    let mut out = String::new();
    let mut error = None;
    unescape(src, Mode::Str, |_, result| match result {
        Ok(c) => out.push(c),
        Err(e) => error = error.or(Some(e)),
    });
    error.map_or(Ok(out), Err)
}

fn text_block(src: &str) -> String {
    let mut out = String::new();
    unescape(src, Mode::TextBlock, |_, result| out.push(result.unwrap()));
    out
}

fn char_lit(src: &str) -> Result<char, EscapeError> {
    let mut value = Err(EscapeError::ZeroChars);
    unescape(src, Mode::Char, |_, result| value = result);
    value
}

#[test]
fn simple_escapes() {
    assert_eq!(
        chars(r#"a\tb\nc\"d\\e\'f\sg"#).unwrap(),
        "a\tb\nc\"d\\e'f g"
    );
    assert_eq!(chars(r"\b\f\r").unwrap(), "\u{8}\u{c}\r");
}

#[test]
fn octal_escapes() {
    assert_eq!(chars(r"\0").unwrap(), "\0");
    assert_eq!(chars(r"\101").unwrap(), "A");
    assert_eq!(chars(r"\377").unwrap(), "\u{ff}");
    assert_eq!(chars(r"\477").unwrap(), "\u{27}7");
}

#[test]
fn unicode_escapes() {
    assert_eq!(chars(r"\u{41}").unwrap(), "A");
    assert_eq!(chars(r"\u{e9}").unwrap(), "é");
    assert_eq!(chars(r"\u{1F600}").unwrap(), "😀");
    assert_eq!(chars(r"\u0041"), Err(EscapeError::InvalidUnicodeEscape));
    assert_eq!(chars(r"\u{}"), Err(EscapeError::InvalidUnicodeEscape));
    assert_eq!(
        chars(r"\u{1234567}"),
        Err(EscapeError::InvalidUnicodeEscape)
    );
    assert_eq!(chars(r"\u{41"), Err(EscapeError::InvalidUnicodeEscape));
    assert_eq!(chars(r"\u{D800}"), Err(EscapeError::SurrogateUnicodeEscape));
    assert_eq!(
        chars(r"\u{110000}"),
        Err(EscapeError::OutOfRangeUnicodeEscape)
    );
}

#[test]
fn invalid_escapes() {
    assert_eq!(chars(r"\q"), Err(EscapeError::InvalidEscape));
    assert_eq!(chars("\\"), Err(EscapeError::LoneSlash));
}

#[test]
fn escape_ranges() {
    let mut ranges = Vec::new();
    unescape(r"a\nb", Mode::Str, |range, _| ranges.push(range));
    assert_eq!(ranges, [0..1, 1..3, 3..4]);
}

#[test]
fn char_literals_hold_one_unicode_scalar() {
    assert_eq!(char_lit("😀"), Ok('😀'));
    assert_eq!(char_lit(r"\u{10FFFF}"), Ok('\u{10FFFF}'));
}

#[test]
fn char_literals() {
    assert_eq!(char_lit("a"), Ok('a'));
    assert_eq!(char_lit(r"\n"), Ok('\n'));
    assert_eq!(char_lit(r"\'"), Ok('\''));
    assert_eq!(char_lit(""), Err(EscapeError::ZeroChars));
    assert_eq!(char_lit("ab"), Err(EscapeError::MoreThanOneChar));
    assert_eq!(char_lit("'"), Err(EscapeError::EscapeOnlyChar));
}

#[test]
fn text_block_line_continuation() {
    assert_eq!(text_block("one \\\ntwo"), "one two");
    assert_eq!(text_block("one \\\r\ntwo"), "one two");
    assert_eq!(text_block("one\ntwo"), "one\ntwo");
}

#[test]
fn strip_indent_with_closing_delimiter_on_own_line() {
    let content = "\n    Hello\n      World\n    ";
    assert_eq!(strip_indent(content), "Hello\n  World\n");
}

#[test]
fn strip_indent_with_closing_delimiter_after_text() {
    let content = "\n    Hello\n    World";
    assert_eq!(strip_indent(content), "Hello\nWorld");
}

#[test]
fn strip_indent_uses_closing_delimiter_indent() {
    let content = "\n    Hello\n  ";
    assert_eq!(strip_indent(content), "  Hello\n");
}

#[test]
fn strip_indent_removes_trailing_whitespace_and_keeps_blank_lines() {
    let content = "\n    a   \n\n    b\n    ";
    assert_eq!(strip_indent(content), "a\n\nb\n");
}

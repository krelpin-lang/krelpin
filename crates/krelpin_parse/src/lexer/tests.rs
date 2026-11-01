use krelpin_ast::token::{Lit, LitKind, Token, TokenKind};
use krelpin_errors::{DiagCtxt, DiagnosticBuffer};
use krelpin_span::{BytePos, SourceMap, Span, Symbol, kw};

use super::lex;

struct Lexed {
    tokens: Vec<Token>,
    errors: Vec<String>,
}

impl Lexed {
    fn kinds(&self) -> Vec<TokenKind> {
        self.tokens.iter().map(|token| token.kind).collect()
    }
}

fn lex_str(src: &str) -> Lexed {
    let source_map = SourceMap::new();
    let file = source_map.new_source_file("test.kr", src.to_owned());
    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());
    let tokens = lex(&file, &dcx);
    Lexed {
        tokens,
        errors: buffer.messages(),
    }
}

fn kinds(src: &str) -> Vec<TokenKind> {
    let lexed = lex_str(src);
    assert_eq!(
        lexed.errors,
        Vec::<String>::new(),
        "unexpected errors for {src:?}"
    );
    lexed.kinds()
}

fn errors(src: &str) -> Vec<String> {
    lex_str(src).errors
}

fn ident(name: &str) -> TokenKind {
    TokenKind::Ident(Symbol::intern(name))
}

fn lit(kind: LitKind, symbol: &str, suffix: Option<&str>) -> TokenKind {
    TokenKind::Literal(Lit::new(
        kind,
        Symbol::intern(symbol),
        suffix.map(Symbol::intern),
    ))
}

fn single_literal(src: &str) -> TokenKind {
    let kinds = kinds(src);
    assert_eq!(kinds.len(), 2, "expected one literal in {src:?}");
    kinds[0]
}

fn span(lo: u32, hi: u32) -> Span {
    Span::new(BytePos(lo), BytePos(hi))
}

#[test]
fn hello_world() {
    use TokenKind::*;
    let src = "package example;\n\
               \n\
               class Main {\n\
               \x20   void main() {\n\
               \x20       Console.println(\"Hello World!\");\n\
               \x20   }\n\
               }\n";
    assert_eq!(
        kinds(src),
        [
            Ident(kw::Package),
            ident("example"),
            Semi,
            Ident(kw::Class),
            ident("Main"),
            OpenBrace,
            Ident(kw::Void),
            ident("main"),
            OpenParen,
            CloseParen,
            OpenBrace,
            ident("Console"),
            Dot,
            ident("println"),
            OpenParen,
            lit(LitKind::Str, "Hello World!", None),
            CloseParen,
            Semi,
            CloseBrace,
            CloseBrace,
            Eof,
        ]
    );
}

#[test]
fn methods_with_receivers() {
    use TokenKind::*;
    assert_eq!(
        kinds("void rename(&mut this, String name)"),
        [
            Ident(kw::Void),
            ident("rename"),
            OpenParen,
            And,
            Ident(kw::Mut),
            Ident(kw::This),
            Comma,
            ident("String"),
            ident("name"),
            CloseParen,
            Eof,
        ]
    );
}

#[test]
fn spans_point_into_source() {
    let lexed = lex_str("  foo = 1;");
    let spans: Vec<Span> = lexed.tokens.iter().map(|token| token.span).collect();
    assert_eq!(
        spans,
        [
            span(2, 5),
            span(6, 7),
            span(8, 9),
            span(9, 10),
            span(10, 10)
        ]
    );
}

#[test]
fn operators_are_glued() {
    use TokenKind::*;
    assert_eq!(kinds("a >>>= b"), [ident("a"), UShrEq, ident("b"), Eof]);
    assert_eq!(kinds("x -> x"), [ident("x"), Arrow, ident("x"), Eof]);
    assert_eq!(kinds("a::b"), [ident("a"), PathSep, ident("b"), Eof]);
    assert_eq!(
        kinds("String... args"),
        [ident("String"), DotDotDot, ident("args"), Eof]
    );
    assert_eq!(
        kinds("i++ <= j--"),
        [ident("i"), PlusPlus, Le, ident("j"), MinusMinus, Eof]
    );
}

#[test]
fn whitespace_and_comments_prevent_gluing() {
    use TokenKind::*;
    assert_eq!(kinds("> >"), [Gt, Gt, Eof]);
    assert_eq!(kinds(">/**/>"), [Gt, Gt, Eof]);
    assert_eq!(kinds(">// x\n>"), [Gt, Gt, Eof]);
}

#[test]
fn reference_to_reference_is_glued_for_parser_to_break() {
    use TokenKind::*;
    assert_eq!(kinds("&&x"), [AndAnd, ident("x"), Eof]);
    assert_eq!(AndAnd.break_two_token_op(), Some((And, And)));
}

#[test]
fn glued_span_covers_all_parts() {
    let lexed = lex_str("a >>>= b");
    assert_eq!(lexed.tokens[1].span, span(2, 6));
}

#[test]
fn comments_are_skipped_and_doc_comments_kept() {
    use TokenKind::*;
    assert_eq!(
        kinds("// line\n/* block */ /** Docs */ class"),
        [DocComment(Symbol::intern(" Docs ")), Ident(kw::Class), Eof]
    );
}

#[test]
fn integer_literals() {
    assert_eq!(single_literal("42"), lit(LitKind::Integer, "42", None));
    assert_eq!(single_literal("0"), lit(LitKind::Integer, "0", None));
    assert_eq!(
        single_literal("100L"),
        lit(LitKind::Integer, "100", Some("L"))
    );
    assert_eq!(single_literal("0xFF"), lit(LitKind::Integer, "0xFF", None));
    assert_eq!(
        single_literal("0b1010"),
        lit(LitKind::Integer, "0b1010", None)
    );
    assert_eq!(
        single_literal("1_000"),
        lit(LitKind::Integer, "1_000", None)
    );
}

#[test]
fn float_literals() {
    assert_eq!(single_literal("3.14"), lit(LitKind::Float, "3.14", None));
    assert_eq!(
        single_literal("3.14f"),
        lit(LitKind::Float, "3.14", Some("f"))
    );
    assert_eq!(single_literal("1f"), lit(LitKind::Float, "1", Some("f")));
    assert_eq!(single_literal("1e-3"), lit(LitKind::Float, "1e-3", None));
    assert_eq!(single_literal(".5"), lit(LitKind::Float, ".5", None));
}

#[test]
fn number_errors() {
    assert_eq!(errors("0x"), ["no valid digits found for number"]);
    assert_eq!(errors("0b102"), ["invalid digit `2` for a base 2 literal"]);
    assert_eq!(
        errors("017"),
        ["integer literals cannot have leading zeros"]
    );
    assert_eq!(errors("1e"), ["expected at least one digit in exponent"]);
    assert_eq!(errors("10px"), ["invalid suffix `px` for number literal"]);
    assert_eq!(errors("0xFFf"), Vec::<String>::new());
    assert_eq!(errors("0x1d"), Vec::<String>::new());
    assert_eq!(errors("1.5L"), ["invalid suffix `L` for float literal"]);
}

#[test]
fn invalid_binary_digit_span() {
    let src = "0b102";
    let source_map = SourceMap::new();
    let file = source_map.new_source_file("test.kr", src.to_owned());
    let buffer = DiagnosticBuffer::default();
    lex(&file, &DiagCtxt::new(buffer.clone()));
    assert_eq!(buffer.diagnostics()[0].span, Some(span(4, 5)));
}

#[test]
fn char_literals() {
    assert_eq!(single_literal("'a'"), lit(LitKind::Char, "a", None));
    assert_eq!(single_literal("'\\n'"), lit(LitKind::Char, "\\n", None));
    assert_eq!(
        single_literal("'\\u0041'"),
        lit(LitKind::Char, "\\u0041", None)
    );
}

#[test]
fn char_errors() {
    assert_eq!(errors("''"), ["empty character literal"]);
    assert_eq!(
        errors("'ab'"),
        ["character literal may only contain one codepoint"]
    );
    assert_eq!(
        errors("'''"),
        ["empty character literal", "unterminated character literal"]
    );
    assert_eq!(errors("'a"), ["unterminated character literal"]);
    assert_eq!(errors("'a'x"), ["character literal cannot have a suffix"]);
}

#[test]
fn string_literals() {
    assert_eq!(
        single_literal(r#""say \"hi\"""#),
        lit(LitKind::Str, r#"say \"hi\""#, None)
    );
    assert_eq!(single_literal(r#""""#), lit(LitKind::Str, "", None));
}

#[test]
fn string_errors() {
    assert_eq!(errors("\"abc"), ["unterminated double quote string"]);
    assert_eq!(errors("\"abc\nx"), ["unterminated double quote string"]);
    assert_eq!(errors(r#""\q""#), ["unknown character escape: `q`"]);
    assert_eq!(errors(r#""\u12""#), ["invalid unicode escape"]);
    assert_eq!(
        errors(r#""\uD800""#),
        ["unicode escape must not be a surrogate"]
    );
}

#[test]
fn escape_error_span() {
    let src = r#"x = "ab\qc";"#;
    let source_map = SourceMap::new();
    let file = source_map.new_source_file("test.kr", src.to_owned());
    let buffer = DiagnosticBuffer::default();
    lex(&file, &DiagCtxt::new(buffer.clone()));
    let escape = src.find('\\').unwrap() as u32;
    assert_eq!(buffer.diagnostics()[0].span, Some(span(escape, escape + 2)));
}

#[test]
fn text_blocks() {
    let src = "\"\"\"\n    Hello\n      \"World\"\n    \"\"\"";
    assert_eq!(
        single_literal(src),
        lit(
            LitKind::TextBlock,
            "\n    Hello\n      \"World\"\n    ",
            None
        )
    );
}

#[test]
fn text_block_errors() {
    assert_eq!(
        errors("\"\"\"Hello\"\"\""),
        ["text block opening delimiter must be followed by a line break"]
    );
    assert_eq!(errors("\"\"\"\nHello"), ["unterminated text block"]);
    assert_eq!(
        errors("\"\"\"\n\\q\n\"\"\""),
        ["unknown character escape: `q`"]
    );
}

#[test]
fn unterminated_block_comment() {
    assert_eq!(errors("class /* oops"), ["unterminated block comment"]);
}

#[test]
fn unknown_characters_are_reported_and_skipped() {
    let lexed = lex_str("a # b");
    assert_eq!(lexed.errors, ["unknown start of token: #"]);
    assert_eq!(lexed.kinds(), [ident("a"), ident("b"), TokenKind::Eof]);
}

#[test]
fn lexing_continues_after_errors() {
    let lexed = lex_str("\"abc\nint x = 0x;");
    assert_eq!(
        lexed.errors,
        [
            "unterminated double quote string",
            "no valid digits found for number"
        ]
    );
    assert_eq!(
        lexed.tokens.last().map(|token| token.kind),
        Some(TokenKind::Eof)
    );
}

#[test]
fn second_file_spans_are_offset() {
    let source_map = SourceMap::new();
    source_map.new_source_file("a.kr", "class".to_owned());
    let file = source_map.new_source_file("b.kr", "x".to_owned());
    let tokens = lex(&file, &DiagCtxt::new(DiagnosticBuffer::default()));
    assert_eq!(tokens[0].span, span(6, 7));
}

#[test]
fn nullable_types_and_operators() {
    use TokenKind::*;
    assert_eq!(
        kinds("String? name = null;"),
        [
            ident("String"),
            Question,
            ident("name"),
            Eq,
            Ident(kw::Null),
            Semi,
            Eof,
        ]
    );
    assert_eq!(
        kinds("name?.length() ?: 0"),
        [
            ident("name"),
            QuestionDot,
            ident("length"),
            OpenParen,
            CloseParen,
            QuestionColon,
            lit(LitKind::Integer, "0", None),
            Eof,
        ]
    );
}

#[test]
fn ternary_is_not_glued() {
    use TokenKind::*;
    assert_eq!(
        kinds("ok ? .5 : 1.5"),
        [
            ident("ok"),
            Question,
            lit(LitKind::Float, ".5", None),
            Colon,
            lit(LitKind::Float, "1.5", None),
            Eof,
        ]
    );
    assert_eq!(
        kinds("List<String?>"),
        [ident("List"), Lt, ident("String"), Question, Gt, Eof]
    );
}

#[test]
fn class_level_fields_and_constructors() {
    use TokenKind::*;
    assert_eq!(
        kinds("shared AtomicInt created = new AtomicInt(0);"),
        [
            Ident(kw::Shared),
            ident("AtomicInt"),
            ident("created"),
            Eq,
            Ident(kw::New),
            ident("AtomicInt"),
            OpenParen,
            lit(LitKind::Integer, "0", None),
            CloseParen,
            Semi,
            Eof,
        ]
    );
    assert_eq!(
        kinds("const int MAX = 64;"),
        [
            Ident(kw::Const),
            Ident(kw::Int),
            ident("MAX"),
            Eq,
            lit(LitKind::Integer, "64", None),
            Semi,
            Eof,
        ]
    );
}

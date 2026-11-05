use crate::{Base, LiteralKind, TokenKind, tokenize};

fn kinds(src: &str) -> Vec<TokenKind> {
    tokenize(src)
        .map(|t| t.kind)
        .filter(|k| *k != TokenKind::Whitespace)
        .collect()
}

fn lit(kind: LiteralKind, suffix_start: u32) -> TokenKind {
    TokenKind::Literal { kind, suffix_start }
}

fn int(base: Base) -> LiteralKind {
    LiteralKind::Int {
        base,
        empty_int: false,
    }
}

fn float() -> LiteralKind {
    LiteralKind::Float {
        empty_exponent: false,
    }
}

fn char_lit(terminated: bool) -> LiteralKind {
    LiteralKind::Char { terminated }
}

fn str_lit(terminated: bool) -> LiteralKind {
    LiteralKind::Str { terminated }
}

fn text_block(terminated: bool) -> LiteralKind {
    LiteralKind::TextBlock { terminated }
}

fn block(doc: bool, terminated: bool) -> TokenKind {
    TokenKind::BlockComment { doc, terminated }
}

#[test]
fn lengths_cover_input() {
    let src = "package example;\n\
               \n\
               /** Entry point */\n\
               class Main {\n\
               \x20   void main() {\n\
               \x20       Console.println(\"Hello, 世界!\"); // output\n\
               \x20   }\n\
               }\n";
    let total: u32 = tokenize(src).map(|t| t.len).sum();
    assert_eq!(total as usize, src.len());
}

#[test]
fn package_and_import() {
    use TokenKind::*;
    assert_eq!(kinds("package example;"), [Ident, Ident, Semi]);
    assert_eq!(
        kinds("import example.test;"),
        [Ident, Ident, Dot, Ident, Semi]
    );
}

#[test]
fn class_and_method() {
    use TokenKind::*;
    assert_eq!(
        kinds("class Main { void main() {} }"),
        [
            Ident, Ident, OpenBrace, Ident, Ident, OpenParen, CloseParen, OpenBrace, CloseBrace,
            CloseBrace,
        ]
    );
}

#[test]
fn borrows_of_this() {
    use TokenKind::*;
    assert_eq!(kinds("&this.name"), [And, Ident, Dot, Ident]);
    assert_eq!(kinds("&mut this.items"), [And, Ident, Ident, Dot, Ident]);
}

#[test]
fn borrow_sources() {
    use TokenKind::*;
    assert_eq!(
        kinds("&String f(&String a) borrows a"),
        [
            And, Ident, Ident, OpenParen, And, Ident, Ident, CloseParen, Ident, Ident
        ]
    );
}

#[test]
fn operators_are_single_characters() {
    use TokenKind::*;
    assert_eq!(kinds("a >>>= b"), [Ident, Gt, Gt, Gt, Eq, Ident]);
    assert_eq!(kinds("x -> x"), [Ident, Minus, Gt, Ident]);
}

#[test]
fn unknown_characters() {
    use TokenKind::*;
    assert_eq!(kinds("# $"), [Unknown, Unknown]);
}

#[test]
fn chars() {
    assert_eq!(kinds("'a'"), [lit(char_lit(true), 3)]);
    assert_eq!(kinds("'\\n'"), [lit(char_lit(true), 4)]);
    assert_eq!(kinds("'\\''"), [lit(char_lit(true), 4)]);
    assert_eq!(kinds("''"), [lit(char_lit(true), 2)]);
    assert_eq!(kinds("'ab'"), [lit(char_lit(true), 4)]);
}

#[test]
fn unterminated_char_stops_at_line_end() {
    use TokenKind::*;
    assert_eq!(kinds("'a\nx"), [lit(char_lit(false), 2), Ident]);
}

#[test]
fn numbers() {
    let empty_hex = LiteralKind::Int {
        base: Base::Hexadecimal,
        empty_int: true,
    };
    assert_eq!(kinds("0x"), [lit(empty_hex, 2)]);
    assert_eq!(kinds("0xFF"), [lit(int(Base::Hexadecimal), 4)]);
    assert_eq!(kinds("0XffL"), [lit(int(Base::Hexadecimal), 4)]);
    assert_eq!(kinds("0b1010"), [lit(int(Base::Binary), 6)]);
    assert_eq!(kinds("0b102"), [lit(int(Base::Binary), 5)]);
    assert_eq!(kinds("1_000_000"), [lit(int(Base::Decimal), 9)]);
    assert_eq!(kinds("100L"), [lit(int(Base::Decimal), 3)]);
    assert_eq!(kinds("1f"), [lit(int(Base::Decimal), 1)]);
}

#[test]
fn floats() {
    assert_eq!(kinds("3.14"), [lit(float(), 4)]);
    assert_eq!(kinds("3.14f"), [lit(float(), 4)]);
    assert_eq!(kinds("1.5e10d"), [lit(float(), 6)]);
    assert_eq!(kinds("1e-3"), [lit(float(), 4)]);
    assert_eq!(kinds(".5"), [lit(float(), 2)]);
    assert_eq!(kinds("1."), [lit(float(), 2)]);
    assert_eq!(
        kinds("1e"),
        [lit(
            LiteralKind::Float {
                empty_exponent: true
            },
            2
        )]
    );
}

#[test]
fn method_call_on_number_is_not_float() {
    use TokenKind::*;
    assert_eq!(
        kinds("1.toString()"),
        [
            lit(int(Base::Decimal), 1),
            Dot,
            Ident,
            OpenParen,
            CloseParen,
        ]
    );
}

#[test]
fn line_comments() {
    use TokenKind::*;
    assert_eq!(kinds("// note"), [LineComment]);
    assert_eq!(kinds("/// still a line comment"), [LineComment]);
    assert_eq!(
        kinds("int x; // note\nint y;"),
        [Ident, Ident, Semi, LineComment, Ident, Ident, Semi]
    );
}

#[test]
fn block_comments() {
    assert_eq!(kinds("/* note */"), [block(false, true)]);
    assert_eq!(kinds("/**/"), [block(false, true)]);
    assert_eq!(kinds("/** Javadoc */"), [block(true, true)]);
    assert_eq!(kinds("/* never closed"), [block(false, false)]);
}

#[test]
fn block_comments_do_not_nest() {
    use TokenKind::*;
    assert_eq!(
        kinds("/* a /* b */ c */"),
        [block(false, true), Ident, Star, Slash]
    );
}

#[test]
fn strings() {
    assert_eq!(kinds(r#""a\"b""#), [lit(str_lit(true), 6)]);
    assert_eq!(kinds(r#""tab\there""#), [lit(str_lit(true), 11)]);
    assert_eq!(kinds(r#""""#), [lit(str_lit(true), 2)]);
    assert_eq!(kinds("\"never closed"), [lit(str_lit(false), 13)]);
}

#[test]
fn strings_do_not_span_lines() {
    use TokenKind::*;
    assert_eq!(kinds("\"abc\nx"), [lit(str_lit(false), 4), Ident]);
    assert_eq!(kinds("\"abc\\\nx"), [lit(str_lit(false), 5), Ident]);
}

#[test]
fn text_blocks() {
    let src = "\"\"\"\n    Hello\n    \"\"\"";
    assert_eq!(kinds(src), [lit(text_block(true), src.len() as u32)]);

    let escaped = "\"\"\"\n  say \\\"\"\"hi\\\"\"\"\n  \"\"\"";
    assert_eq!(
        kinds(escaped),
        [lit(text_block(true), escaped.len() as u32)]
    );

    assert_eq!(kinds("\"\"\"\nopen"), [lit(text_block(false), 8)]);
}

#[test]
fn interpolations_keep_strings_whole() {
    for literal in [
        r#""a ${f("x")} b""#,
        r#""${m.get("}")}""#,
        r#""${"${"deep"}"}""#,
        r#""${'}'}""#,
    ] {
        let src = format!("{literal} x");
        assert_eq!(
            kinds(&src),
            [lit(str_lit(true), literal.len() as u32), TokenKind::Ident],
            "{literal}"
        );
    }
    assert_eq!(
        kinds("\"${a\nb}\""),
        [
            lit(str_lit(false), 4),
            TokenKind::Ident,
            TokenKind::CloseBrace,
            lit(str_lit(false), 1),
        ]
    );
}

mod interpolation {
    use crate::interpolate::{ErrorKind, Piece, parse_spec, pieces, spec_colon, texts};
    use crate::unescape::Mode;

    #[test]
    fn splits_text_names_and_expressions() {
        assert_eq!(
            pieces("Hi $name, ${a + b}!"),
            Ok(vec![
                Piece::Text(0..3),
                Piece::Name(4..8),
                Piece::Text(8..10),
                Piece::Expr(12..17),
                Piece::Text(18..19),
            ])
        );
        assert_eq!(
            pieces("cost \\$5 and $ alone"),
            Ok(vec![Piece::Text(0..20)])
        );
        assert_eq!(pieces(r#"${f("}")}"#), Ok(vec![Piece::Expr(2..8)]));
        assert_eq!(pieces("x ${ }").map_err(|e| e.kind), Err(ErrorKind::Empty));
        assert_eq!(
            pieces("x ${a").map_err(|e| e.kind),
            Err(ErrorKind::Unclosed)
        );
    }

    #[test]
    fn cooks_the_text_between_interpolations() {
        let content = "a\\t${x}\\$b$y";
        let pieces = pieces(content).expect("valid");
        assert_eq!(texts(content, &pieces, Mode::Str), ["a\t", "$b", ""]);
        let block = "\n    head ${x}\n      tail\n    ";
        let pieces = crate::interpolate::pieces(block).expect("valid");
        assert_eq!(
            texts(block, &pieces, Mode::TextBlock),
            ["head ", "\n  tail\n"]
        );
    }

    #[test]
    fn finds_the_format_colon() {
        assert_eq!(spec_colon("price:.2"), Some(5));
        assert_eq!(spec_colon("a ? b : c"), None);
        assert_eq!(spec_colon("a ? b : c:>4"), Some(9));
        assert_eq!(spec_colon("x ?: y"), None);
        assert_eq!(spec_colon("List::of"), None);
        assert_eq!(spec_colon(r#"m.get(":")"#), None);
        assert_eq!(spec_colon("f(a ? b : c):x"), Some(12));
    }

    #[test]
    fn parses_format_specs() {
        let spec = parse_spec("*^+#010.3e").expect("valid");
        assert_eq!(spec.fill, Some('*'));
        assert_eq!(spec.align, Some('^'));
        assert!(spec.plus && spec.alternate && spec.zero);
        assert_eq!(spec.width, Some(10));
        assert_eq!(spec.precision, Some(3));
        assert_eq!(spec.kind, Some('e'));
        assert_eq!(parse_spec(">8").map(|spec| spec.width), Ok(Some(8)));
        assert_eq!(parse_spec("").map(|spec| spec.width), Ok(None));
        assert!(parse_spec(".").is_err());
        assert!(parse_spec("q").is_err());
        assert!(parse_spec("5x!").is_err());
        assert!(parse_spec("99999").is_err());
    }
}

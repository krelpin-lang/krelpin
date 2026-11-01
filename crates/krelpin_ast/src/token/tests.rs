use krelpin_span::{BytePos, Span, Symbol, kw};

use super::{Token, TokenKind};

const PUNCTUATION: &[TokenKind] = &[
    TokenKind::Eq,
    TokenKind::EqEq,
    TokenKind::Ne,
    TokenKind::Lt,
    TokenKind::Le,
    TokenKind::Gt,
    TokenKind::Ge,
    TokenKind::AndAnd,
    TokenKind::OrOr,
    TokenKind::Bang,
    TokenKind::Tilde,
    TokenKind::Plus,
    TokenKind::Minus,
    TokenKind::Star,
    TokenKind::Slash,
    TokenKind::Percent,
    TokenKind::Caret,
    TokenKind::And,
    TokenKind::Or,
    TokenKind::Shl,
    TokenKind::Shr,
    TokenKind::UShr,
    TokenKind::PlusEq,
    TokenKind::MinusEq,
    TokenKind::StarEq,
    TokenKind::SlashEq,
    TokenKind::PercentEq,
    TokenKind::CaretEq,
    TokenKind::AndEq,
    TokenKind::OrEq,
    TokenKind::ShlEq,
    TokenKind::ShrEq,
    TokenKind::UShrEq,
    TokenKind::PlusPlus,
    TokenKind::MinusMinus,
    TokenKind::At,
    TokenKind::Dot,
    TokenKind::DotDot,
    TokenKind::DotDotDot,
    TokenKind::Comma,
    TokenKind::Semi,
    TokenKind::Colon,
    TokenKind::PathSep,
    TokenKind::Arrow,
    TokenKind::Question,
    TokenKind::QuestionDot,
    TokenKind::QuestionColon,
    TokenKind::OpenParen,
    TokenKind::CloseParen,
    TokenKind::OpenBrace,
    TokenKind::CloseBrace,
    TokenKind::OpenBracket,
    TokenKind::CloseBracket,
];

fn text(kind: TokenKind) -> &'static str {
    kind.as_str().unwrap()
}

#[test]
fn glue_concatenates_text() {
    for &first in PUNCTUATION {
        for &second in PUNCTUATION {
            if let Some(glued) = first.glue(second) {
                assert_eq!(text(glued), format!("{}{}", text(first), text(second)));
            }
        }
    }
}

#[test]
fn break_splits_text() {
    for &kind in PUNCTUATION {
        if let Some((first, second)) = kind.break_two_token_op() {
            assert_eq!(text(kind), format!("{}{}", text(first), text(second)));
        }
    }
}

#[test]
fn every_multi_char_operator_can_be_broken() {
    for &kind in PUNCTUATION {
        let is_compound = text(kind).len() > 1;
        assert_eq!(kind.break_two_token_op().is_some(), is_compound, "{kind:?}");
    }
}

#[test]
fn shift_breaks_into_generic_closers() {
    assert_eq!(
        TokenKind::Shr.break_two_token_op(),
        Some((TokenKind::Gt, TokenKind::Gt))
    );
    assert_eq!(
        TokenKind::AndAnd.break_two_token_op(),
        Some((TokenKind::And, TokenKind::And))
    );
}

#[test]
fn glued_token_spans_both() {
    let span = |lo, hi| Span::new(BytePos(lo), BytePos(hi));
    let minus = Token::new(TokenKind::Minus, span(4, 5));
    let gt = Token::new(TokenKind::Gt, span(5, 6));
    assert_eq!(
        minus.glue(&gt),
        Some(Token::new(TokenKind::Arrow, span(4, 6)))
    );
}

#[test]
fn keywords_are_not_plain_idents() {
    let this = Token::new(TokenKind::Ident(kw::This), Span::DUMMY);
    assert!(this.is_keyword(kw::This));
    assert_eq!(this.ident(), None);

    let name = Symbol::intern("name");
    let ident = Token::new(TokenKind::Ident(name), Span::DUMMY);
    assert_eq!(ident.ident(), Some(name));
}

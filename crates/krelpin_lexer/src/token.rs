#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub len: u32,
}

impl Token {
    pub(crate) fn new(kind: TokenKind, len: u32) -> Token {
        Token { kind, len }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    LineComment,
    BlockComment {
        doc: bool,
        terminated: bool,
    },
    Whitespace,
    Ident,
    Literal {
        kind: LiteralKind,
        suffix_start: u32,
    },
    Semi,
    Comma,
    Dot,
    OpenParen,
    CloseParen,
    OpenBrace,
    CloseBrace,
    OpenBracket,
    CloseBracket,
    At,
    Tilde,
    Question,
    Colon,
    Eq,
    Bang,
    Lt,
    Gt,
    Minus,
    And,
    Or,
    Plus,
    Star,
    Slash,
    Caret,
    Percent,
    Unknown,
    Eof,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiteralKind {
    Int { base: Base, empty_int: bool },
    Float { empty_exponent: bool },
    Char { terminated: bool },
    Str { terminated: bool },
    TextBlock { terminated: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Binary = 2,
    Decimal = 10,
    Hexadecimal = 16,
}

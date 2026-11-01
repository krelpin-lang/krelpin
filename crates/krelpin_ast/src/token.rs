use krelpin_span::{Span, Symbol};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LitKind {
    Integer,
    Float,
    Char,
    Str,
    TextBlock,
    Err,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lit {
    pub kind: LitKind,
    pub symbol: Symbol,
    pub suffix: Option<Symbol>,
}

impl Lit {
    pub fn new(kind: LitKind, symbol: Symbol, suffix: Option<Symbol>) -> Lit {
        Lit {
            kind,
            symbol,
            suffix,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Eq,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Bang,
    Tilde,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    And,
    Or,
    Shl,
    Shr,
    UShr,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    CaretEq,
    AndEq,
    OrEq,
    ShlEq,
    ShrEq,
    UShrEq,
    PlusPlus,
    MinusMinus,
    At,
    Dot,
    DotDot,
    DotDotDot,
    Comma,
    Semi,
    Colon,
    PathSep,
    Arrow,
    Question,
    QuestionDot,
    QuestionColon,
    OpenParen,
    CloseParen,
    OpenBrace,
    CloseBrace,
    OpenBracket,
    CloseBracket,
    Literal(Lit),
    Ident(Symbol),
    DocComment(Symbol),
    Eof,
}

impl TokenKind {
    pub fn glue(self, next: TokenKind) -> Option<TokenKind> {
        use TokenKind::*;

        let glued = match (self, next) {
            (Eq, Eq) => EqEq,
            (Bang, Eq) => Ne,
            (Lt, Eq) => Le,
            (Lt, Lt) => Shl,
            (Shl, Eq) => ShlEq,
            (Gt, Eq) => Ge,
            (Gt, Gt) => Shr,
            (Shr, Gt) => UShr,
            (Shr, Eq) => ShrEq,
            (UShr, Eq) => UShrEq,
            (And, And) => AndAnd,
            (And, Eq) => AndEq,
            (Or, Or) => OrOr,
            (Or, Eq) => OrEq,
            (Plus, Plus) => PlusPlus,
            (Plus, Eq) => PlusEq,
            (Minus, Minus) => MinusMinus,
            (Minus, Eq) => MinusEq,
            (Minus, Gt) => Arrow,
            (Star, Eq) => StarEq,
            (Slash, Eq) => SlashEq,
            (Percent, Eq) => PercentEq,
            (Caret, Eq) => CaretEq,
            (Colon, Colon) => PathSep,
            (Dot, Dot) => DotDot,
            (Question, Dot) => QuestionDot,
            (Question, Colon) => QuestionColon,
            (DotDot, Dot) => DotDotDot,
            _ => return None,
        };
        Some(glued)
    }

    pub fn break_two_token_op(self) -> Option<(TokenKind, TokenKind)> {
        use TokenKind::*;

        let parts = match self {
            EqEq => (Eq, Eq),
            Ne => (Bang, Eq),
            Le => (Lt, Eq),
            Shl => (Lt, Lt),
            ShlEq => (Lt, Le),
            Ge => (Gt, Eq),
            Shr => (Gt, Gt),
            UShr => (Gt, Shr),
            ShrEq => (Gt, Ge),
            UShrEq => (Gt, ShrEq),
            AndAnd => (And, And),
            AndEq => (And, Eq),
            OrOr => (Or, Or),
            OrEq => (Or, Eq),
            PlusPlus => (Plus, Plus),
            PlusEq => (Plus, Eq),
            MinusMinus => (Minus, Minus),
            MinusEq => (Minus, Eq),
            Arrow => (Minus, Gt),
            StarEq => (Star, Eq),
            SlashEq => (Slash, Eq),
            PercentEq => (Percent, Eq),
            CaretEq => (Caret, Eq),
            PathSep => (Colon, Colon),
            DotDot => (Dot, Dot),
            DotDotDot => (Dot, DotDot),
            QuestionDot => (Question, Dot),
            QuestionColon => (Question, Colon),
            _ => return None,
        };
        Some(parts)
    }

    pub fn as_str(self) -> Option<&'static str> {
        use TokenKind::*;

        let text = match self {
            Eq => "=",
            EqEq => "==",
            Ne => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
            AndAnd => "&&",
            OrOr => "||",
            Bang => "!",
            Tilde => "~",
            Plus => "+",
            Minus => "-",
            Star => "*",
            Slash => "/",
            Percent => "%",
            Caret => "^",
            And => "&",
            Or => "|",
            Shl => "<<",
            Shr => ">>",
            UShr => ">>>",
            PlusEq => "+=",
            MinusEq => "-=",
            StarEq => "*=",
            SlashEq => "/=",
            PercentEq => "%=",
            CaretEq => "^=",
            AndEq => "&=",
            OrEq => "|=",
            ShlEq => "<<=",
            ShrEq => ">>=",
            UShrEq => ">>>=",
            PlusPlus => "++",
            MinusMinus => "--",
            At => "@",
            Dot => ".",
            DotDot => "..",
            DotDotDot => "...",
            Comma => ",",
            Semi => ";",
            Colon => ":",
            PathSep => "::",
            Arrow => "->",
            Question => "?",
            QuestionDot => "?.",
            QuestionColon => "?:",
            OpenParen => "(",
            CloseParen => ")",
            OpenBrace => "{",
            CloseBrace => "}",
            OpenBracket => "[",
            CloseBracket => "]",
            Literal(_) | Ident(_) | DocComment(_) | Eof => return None,
        };
        Some(text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Token {
        Token { kind, span }
    }

    pub fn glue(&self, next: &Token) -> Option<Token> {
        let kind = self.kind.glue(next.kind)?;
        Some(Token::new(kind, self.span.to(next.span)))
    }

    pub fn is_keyword(&self, keyword: Symbol) -> bool {
        self.kind == TokenKind::Ident(keyword)
    }

    pub fn ident(&self) -> Option<Symbol> {
        match self.kind {
            TokenKind::Ident(symbol) if !symbol.is_keyword() => Some(symbol),
            _ => None,
        }
    }
}

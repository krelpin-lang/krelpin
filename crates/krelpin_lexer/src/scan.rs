use crate::cursor::Cursor;
use crate::token::{Token, TokenKind};
use crate::{is_id_continue, is_id_start, is_whitespace};

impl Cursor<'_> {
    pub fn advance_token(&mut self) -> Token {
        let Some(first) = self.bump() else {
            return Token::new(TokenKind::Eof, 0);
        };

        let kind = match first {
            '/' if self.first() == '/' => self.line_comment(),
            '/' if self.first() == '*' => self.block_comment(),
            c if is_whitespace(c) => self.whitespace(),
            c if is_id_start(c) => self.ident(),
            c @ '0'..='9' => self.number(c),
            '.' if self.first().is_ascii_digit() => self.fraction(),
            '\'' => self.char_literal(),
            '"' => self.string_literal(),
            c => punct(c).unwrap_or(TokenKind::Unknown),
        };

        let token = Token::new(kind, self.pos_within_token());
        self.reset_pos_within_token();
        token
    }

    fn line_comment(&mut self) -> TokenKind {
        self.eat_while(|c| c != '\n');
        TokenKind::LineComment
    }

    fn block_comment(&mut self) -> TokenKind {
        self.bump();
        let doc = self.first() == '*' && self.second() != '/';

        while let Some(c) = self.bump() {
            if c == '*' && self.first() == '/' {
                self.bump();
                return TokenKind::BlockComment {
                    doc,
                    terminated: true,
                };
            }
        }

        TokenKind::BlockComment {
            doc,
            terminated: false,
        }
    }

    fn whitespace(&mut self) -> TokenKind {
        self.eat_while(is_whitespace);
        TokenKind::Whitespace
    }

    fn ident(&mut self) -> TokenKind {
        self.eat_while(is_id_continue);
        TokenKind::Ident
    }
}

fn punct(c: char) -> Option<TokenKind> {
    let kind = match c {
        ';' => TokenKind::Semi,
        ',' => TokenKind::Comma,
        '.' => TokenKind::Dot,
        '(' => TokenKind::OpenParen,
        ')' => TokenKind::CloseParen,
        '{' => TokenKind::OpenBrace,
        '}' => TokenKind::CloseBrace,
        '[' => TokenKind::OpenBracket,
        ']' => TokenKind::CloseBracket,
        '@' => TokenKind::At,
        '~' => TokenKind::Tilde,
        '?' => TokenKind::Question,
        ':' => TokenKind::Colon,
        '=' => TokenKind::Eq,
        '!' => TokenKind::Bang,
        '<' => TokenKind::Lt,
        '>' => TokenKind::Gt,
        '-' => TokenKind::Minus,
        '&' => TokenKind::And,
        '|' => TokenKind::Or,
        '+' => TokenKind::Plus,
        '*' => TokenKind::Star,
        '/' => TokenKind::Slash,
        '^' => TokenKind::Caret,
        '%' => TokenKind::Percent,
        _ => return None,
    };
    Some(kind)
}

use crate::cursor::{Cursor, EOF_CHAR};
use crate::token::{Base, LiteralKind, TokenKind};
use crate::{is_id_continue, is_id_start};

impl Cursor<'_> {
    pub(crate) fn number(&mut self, first_digit: char) -> TokenKind {
        let kind = match (first_digit, self.first()) {
            ('0', 'b' | 'B') => self.prefixed_int(Base::Binary),
            ('0', 'x' | 'X') => self.prefixed_int(Base::Hexadecimal),
            _ => self.decimal(),
        };
        self.finish_literal(kind)
    }

    pub(crate) fn fraction(&mut self) -> TokenKind {
        self.eat_digits(|c| c.is_ascii_digit());
        let kind = self.exponent(true);
        self.finish_literal(kind)
    }

    pub(crate) fn char_literal(&mut self) -> TokenKind {
        let terminated = self.quoted('\'');
        self.finish_literal(LiteralKind::Char { terminated })
    }

    pub(crate) fn string_literal(&mut self) -> TokenKind {
        let kind = if self.first() == '"' && self.second() == '"' {
            LiteralKind::TextBlock {
                terminated: self.text_block(),
            }
        } else {
            LiteralKind::Str {
                terminated: self.quoted('"'),
            }
        };
        self.finish_literal(kind)
    }

    fn prefixed_int(&mut self, base: Base) -> LiteralKind {
        self.bump();
        let has_digits = match base {
            Base::Hexadecimal => self.eat_digits(|c| c.is_ascii_hexdigit()),
            Base::Binary | Base::Decimal => self.eat_digits(|c| c.is_ascii_digit()),
        };
        LiteralKind::Int {
            base,
            empty_int: !has_digits,
        }
    }

    fn decimal(&mut self) -> LiteralKind {
        self.eat_digits(|c| c.is_ascii_digit());

        let has_fraction =
            self.first() == '.' && self.second() != '.' && !is_id_start(self.second());
        if has_fraction {
            self.bump();
            self.eat_digits(|c| c.is_ascii_digit());
        }

        self.exponent(has_fraction)
    }

    fn exponent(&mut self, has_fraction: bool) -> LiteralKind {
        if !matches!(self.first(), 'e' | 'E') {
            return if has_fraction {
                LiteralKind::Float {
                    empty_exponent: false,
                }
            } else {
                LiteralKind::Int {
                    base: Base::Decimal,
                    empty_int: false,
                }
            };
        }

        self.bump();
        if matches!(self.first(), '+' | '-') {
            self.bump();
        }
        let has_digits = self.eat_digits(|c| c.is_ascii_digit());
        LiteralKind::Float {
            empty_exponent: !has_digits,
        }
    }

    fn eat_digits(&mut self, is_digit: impl Fn(char) -> bool) -> bool {
        let mut has_digits = false;
        loop {
            match self.first() {
                '_' => {}
                c if is_digit(c) => has_digits = true,
                _ => return has_digits,
            }
            self.bump();
        }
    }

    fn quoted(&mut self, quote: char) -> bool {
        loop {
            match self.first() {
                c if c == quote => {
                    self.bump();
                    return true;
                }
                '\n' => return false,
                EOF_CHAR if self.is_eof() => return false,
                '\\' => {
                    self.bump();
                    if self.first() != '\n' {
                        self.bump();
                    }
                }
                '$' if quote == '"' && self.second() == '{' => {
                    self.bump();
                    if !self.interpolation(false) {
                        return false;
                    }
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn text_block(&mut self) -> bool {
        self.bump();
        self.bump();

        while let Some(c) = self.bump() {
            match c {
                '"' if self.first() == '"' && self.second() == '"' => {
                    self.bump();
                    self.bump();
                    return true;
                }
                '\\' => {
                    self.bump();
                }
                '$' if self.first() == '{' && !self.interpolation(true) => return false,
                _ => {}
            }
        }
        false
    }

    fn interpolation(&mut self, multiline: bool) -> bool {
        self.bump();
        let mut depth = 1usize;
        loop {
            match self.first() {
                EOF_CHAR if self.is_eof() => return false,
                '\n' if !multiline => return false,
                '{' => {
                    depth += 1;
                    self.bump();
                }
                '}' => {
                    self.bump();
                    depth -= 1;
                    if depth == 0 {
                        return true;
                    }
                }
                '"' => {
                    self.bump();
                    let closed = if self.first() == '"' && self.second() == '"' {
                        self.text_block()
                    } else {
                        self.quoted('"')
                    };
                    if !closed {
                        return false;
                    }
                }
                '\'' => {
                    self.bump();
                    if !self.quoted('\'') {
                        return false;
                    }
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn finish_literal(&mut self, kind: LiteralKind) -> TokenKind {
        let suffix_start = self.pos_within_token();
        if is_id_start(self.first()) {
            self.bump();
            self.eat_while(is_id_continue);
        }
        TokenKind::Literal { kind, suffix_start }
    }
}

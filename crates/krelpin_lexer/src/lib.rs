mod cursor;
pub mod interpolate;
mod literal;
mod scan;
mod token;
pub mod unescape;

#[cfg(test)]
mod tests;

pub use crate::cursor::Cursor;
pub use crate::token::{Base, LiteralKind, Token, TokenKind};

pub fn tokenize(input: &str) -> impl Iterator<Item = Token> {
    let mut cursor = Cursor::new(input);
    std::iter::from_fn(move || {
        let token = cursor.advance_token();
        (token.kind != TokenKind::Eof).then_some(token)
    })
}

pub fn is_whitespace(c: char) -> bool {
    matches!(c, '\t'..='\r' | ' ' | '\u{85}' | '\u{200E}'..='\u{200F}' | '\u{2028}'..='\u{2029}')
}

pub fn is_id_start(c: char) -> bool {
    c == '_' || unicode_ident::is_xid_start(c)
}

pub fn is_id_continue(c: char) -> bool {
    unicode_ident::is_xid_continue(c)
}

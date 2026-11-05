use std::ops::Range;

use crate::unescape::{self, Mode};
use crate::{is_id_continue, is_id_start};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(Range<usize>),
    Expr(Range<usize>),
    Name(Range<usize>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Unclosed,
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub range: Range<usize>,
    pub kind: ErrorKind,
}

pub fn pieces(content: &str) -> Result<Vec<Piece>, Error> {
    let bytes = content.as_bytes();
    let mut pieces = Vec::new();
    let mut text_start = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at = next_char(content, at + 1),
            b'$' if bytes.get(at + 1) == Some(&b'{') => {
                let Some(close) = skip_braces(bytes, at + 2) else {
                    return Err(Error {
                        range: at..content.len(),
                        kind: ErrorKind::Unclosed,
                    });
                };
                if content[at + 2..close].trim().is_empty() {
                    return Err(Error {
                        range: at..close + 1,
                        kind: ErrorKind::Empty,
                    });
                }
                push_text(&mut pieces, text_start..at);
                pieces.push(Piece::Expr(at + 2..close));
                at = close + 1;
                text_start = at;
            }
            b'$' if content[at + 1..].chars().next().is_some_and(is_id_start) => {
                let start = at + 1;
                let end = content[start..]
                    .char_indices()
                    .find(|&(_, c)| !is_id_continue(c))
                    .map_or(content.len(), |(offset, _)| start + offset);
                push_text(&mut pieces, text_start..at);
                pieces.push(Piece::Name(start..end));
                at = end;
                text_start = at;
            }
            _ => at = next_char(content, at),
        }
    }
    push_text(&mut pieces, text_start..content.len());
    Ok(pieces)
}

pub fn is_interpolated(content: &str) -> bool {
    match pieces(content) {
        Ok(pieces) => pieces.iter().any(|piece| !matches!(piece, Piece::Text(_))),
        Err(_) => true,
    }
}

pub fn texts(content: &str, pieces: &[Piece], mode: Mode) -> Vec<String> {
    let marker = (0xE000u32..=0xF8FF)
        .filter_map(char::from_u32)
        .find(|c| !content.contains(*c))
        .unwrap_or('\u{FFFF}');
    let mut masked = String::with_capacity(content.len());
    for piece in pieces {
        match piece {
            Piece::Text(range) => masked.push_str(&content[range.clone()]),
            Piece::Expr(_) | Piece::Name(_) => masked.push(marker),
        }
    }
    let masked = if mode == Mode::TextBlock {
        unescape::strip_indent(&masked)
    } else {
        masked
    };
    masked
        .split(marker)
        .map(|raw| {
            let mut value = String::with_capacity(raw.len());
            unescape::unescape(raw, mode, |_, result| {
                if let Ok(c) = result {
                    value.push(c);
                }
            });
            value
        })
        .collect()
}

pub fn spec_colon(expr: &str) -> Option<usize> {
    let bytes = expr.as_bytes();
    let mut depth = 0usize;
    let mut questions = 0usize;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'"' if bytes[at..].starts_with(b"\"\"\"") => {
                at = skip_text_block(bytes, at + 3)?;
                continue;
            }
            b'"' | b'\'' => {
                at = skip_quoted(bytes, at + 1, bytes[at])?;
                continue;
            }
            b'?' if matches!(bytes.get(at + 1), Some(b'.' | b':')) => {
                at += 2;
                continue;
            }
            b'?' if depth == 0 => questions += 1,
            b':' if bytes.get(at + 1) == Some(&b':') => {
                at += 2;
                continue;
            }
            b':' if depth == 0 => {
                if questions == 0 {
                    return Some(at);
                }
                questions -= 1;
            }
            _ => {}
        }
        at += 1;
    }
    None
}

fn push_text(pieces: &mut Vec<Piece>, range: Range<usize>) {
    if !range.is_empty() {
        pieces.push(Piece::Text(range));
    }
}

fn next_char(content: &str, at: usize) -> usize {
    content[at.min(content.len())..]
        .chars()
        .next()
        .map_or(content.len(), |c| at + c.len_utf8())
}

fn skip_braces(bytes: &[u8], mut at: usize) -> Option<usize> {
    let mut depth = 1usize;
    while at < bytes.len() {
        match bytes[at] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            b'"' if bytes[at..].starts_with(b"\"\"\"") => {
                at = skip_text_block(bytes, at + 3)?;
                continue;
            }
            b'"' => {
                at = skip_quoted(bytes, at + 1, b'"')?;
                continue;
            }
            b'\'' => {
                at = skip_quoted(bytes, at + 1, b'\'')?;
                continue;
            }
            _ => {}
        }
        at += 1;
    }
    None
}

fn skip_quoted(bytes: &[u8], mut at: usize, quote: u8) -> Option<usize> {
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'$' if quote == b'"' && bytes.get(at + 1) == Some(&b'{') => {
                at = skip_braces(bytes, at + 2)? + 1;
            }
            c if c == quote => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
}

fn skip_text_block(bytes: &[u8], mut at: usize) -> Option<usize> {
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'$' if bytes.get(at + 1) == Some(&b'{') => {
                at = skip_braces(bytes, at + 2)? + 1;
            }
            b'"' if bytes[at..].starts_with(b"\"\"\"") => return Some(at + 3),
            _ => at += 1,
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spec {
    pub fill: Option<char>,
    pub align: Option<char>,
    pub plus: bool,
    pub alternate: bool,
    pub zero: bool,
    pub width: Option<usize>,
    pub precision: Option<usize>,
    pub kind: Option<char>,
}

pub fn parse_spec(spec: &str) -> Result<Spec, String> {
    let mut parsed = Spec::default();
    let chars: Vec<char> = spec.chars().collect();
    let mut at = 0;
    let is_align = |c: char| matches!(c, '<' | '^' | '>');
    if chars.len() >= 2 && is_align(chars[1]) {
        parsed.fill = Some(chars[0]);
        parsed.align = Some(chars[1]);
        at = 2;
    } else if chars.first().copied().is_some_and(is_align) {
        parsed.align = Some(chars[0]);
        at = 1;
    }
    if chars.get(at) == Some(&'+') {
        parsed.plus = true;
        at += 1;
    }
    if chars.get(at) == Some(&'#') {
        parsed.alternate = true;
        at += 1;
    }
    if chars.get(at) == Some(&'0') {
        parsed.zero = true;
        at += 1;
    }
    let digits = |at: &mut usize| -> Option<usize> {
        let start = *at;
        while chars.get(*at).is_some_and(char::is_ascii_digit) {
            *at += 1;
        }
        (*at > start).then(|| {
            chars[start..*at]
                .iter()
                .collect::<String>()
                .parse()
                .unwrap_or(usize::MAX)
        })
    };
    parsed.width = digits(&mut at);
    if chars.get(at) == Some(&'.') {
        at += 1;
        parsed.precision = digits(&mut at);
        if parsed.precision.is_none() {
            return Err("expected digits after `.`".to_owned());
        }
    }
    if let Some(&kind) = chars.get(at) {
        if !matches!(kind, 'x' | 'X' | 'b' | 'o' | 'e' | 'E') {
            return Err(format!(
                "unknown format `{kind}`; expected `x`, `X`, `b`, `o`, `e` or `E`"
            ));
        }
        parsed.kind = Some(kind);
        at += 1;
    }
    if at != chars.len() {
        return Err(format!(
            "unexpected `{}`",
            chars[at..].iter().collect::<String>()
        ));
    }
    if parsed.width.is_some_and(|width| width > 4096)
        || parsed.precision.is_some_and(|precision| precision > 4096)
    {
        return Err("widths and precisions go up to 4096".to_owned());
    }
    Ok(parsed)
}

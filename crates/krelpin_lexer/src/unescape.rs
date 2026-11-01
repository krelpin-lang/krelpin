use std::ops::Range;
use std::str::Chars;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Char,
    Str,
    TextBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeError {
    ZeroChars,
    MoreThanOneChar,
    LoneSlash,
    InvalidEscape,
    EscapeOnlyChar,
    InvalidUnicodeEscape,
    SurrogateUnicodeEscape,
}

pub fn unescape(
    src: &str,
    mode: Mode,
    mut callback: impl FnMut(Range<usize>, Result<char, EscapeError>),
) {
    match mode {
        Mode::Char => callback(0..src.len(), unescape_char(src)),
        Mode::Str | Mode::TextBlock => unescape_str(src, mode, &mut callback),
    }
}

pub fn strip_indent(content: &str) -> String {
    let body = content.split_once('\n').map_or("", |(_, rest)| rest);
    let lines: Vec<&str> = body
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let last = lines.len() - 1;

    let indent = lines
        .iter()
        .enumerate()
        .filter(|&(index, line)| index == last || !is_blank(line))
        .map(|(_, line)| leading_whitespace(line))
        .min()
        .unwrap_or(0);

    lines
        .iter()
        .map(|line| {
            if is_blank(line) {
                ""
            } else {
                line[indent..].trim_end_matches(is_indent_char)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn unescape_char(src: &str) -> Result<char, EscapeError> {
    let mut chars = src.chars();
    let c = match chars.next().ok_or(EscapeError::ZeroChars)? {
        '\\' => scan_escape(&mut chars)?,
        '\'' => return Err(EscapeError::EscapeOnlyChar),
        c => c,
    };
    if chars.as_str().is_empty() {
        Ok(c)
    } else {
        Err(EscapeError::MoreThanOneChar)
    }
}

fn unescape_str(
    src: &str,
    mode: Mode,
    callback: &mut impl FnMut(Range<usize>, Result<char, EscapeError>),
) {
    let mut chars = src.chars();
    while let Some(c) = chars.next() {
        let start = src.len() - chars.as_str().len() - c.len_utf8();
        let result = match c {
            '\\' if mode == Mode::TextBlock && skip_line_break(&mut chars) => continue,
            '\\' => scan_escape(&mut chars),
            c => Ok(c),
        };
        let end = src.len() - chars.as_str().len();
        callback(start..end, result);
    }
}

fn skip_line_break(chars: &mut Chars<'_>) -> bool {
    let rest = chars.as_str();
    let skip = if rest.starts_with("\r\n") {
        2
    } else if rest.starts_with(['\n', '\r']) {
        1
    } else {
        return false;
    };
    for _ in 0..skip {
        chars.next();
    }
    true
}

fn scan_escape(chars: &mut Chars<'_>) -> Result<char, EscapeError> {
    let c = chars.next().ok_or(EscapeError::LoneSlash)?;
    let value = match c {
        'b' => '\u{8}',
        't' => '\t',
        'n' => '\n',
        'f' => '\u{c}',
        'r' => '\r',
        's' => ' ',
        '"' => '"',
        '\'' => '\'',
        '\\' => '\\',
        'u' => return scan_unicode(chars),
        '0'..='7' => scan_octal(c, chars),
        _ => return Err(EscapeError::InvalidEscape),
    };
    Ok(value)
}

fn scan_unicode(chars: &mut Chars<'_>) -> Result<char, EscapeError> {
    while chars.as_str().starts_with('u') {
        chars.next();
    }

    let mut value = 0;
    for _ in 0..4 {
        let digit = chars
            .as_str()
            .chars()
            .next()
            .and_then(|c| c.to_digit(16))
            .ok_or(EscapeError::InvalidUnicodeEscape)?;
        chars.next();
        value = value * 16 + digit;
    }

    char::from_u32(value).ok_or(EscapeError::SurrogateUnicodeEscape)
}

fn scan_octal(first: char, chars: &mut Chars<'_>) -> char {
    let max_len = if first <= '3' { 3 } else { 2 };
    let mut value = first as u32 - '0' as u32;

    for _ in 1..max_len {
        match chars.as_str().chars().next().and_then(|c| c.to_digit(8)) {
            Some(digit) => {
                chars.next();
                value = value * 8 + digit;
            }
            None => break,
        }
    }

    char::from(value as u8)
}

fn is_indent_char(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\u{c}')
}

fn is_blank(line: &str) -> bool {
    line.chars().all(is_indent_char)
}

fn leading_whitespace(line: &str) -> usize {
    line.len() - line.trim_start_matches(is_indent_char).len()
}

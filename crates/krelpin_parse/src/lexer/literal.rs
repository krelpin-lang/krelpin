use std::ops::Range;

use krelpin_ast::token::{Lit, LitKind};
use krelpin_errors::Diagnostic;
use krelpin_lexer::unescape::{self, EscapeError, Mode};
use krelpin_lexer::{Base, LiteralKind};
use krelpin_span::{BytePos, Span, Symbol};

use super::StringReader;

const VALID_ESCAPES: &str = "valid escapes are `\\b`, `\\t`, `\\n`, `\\f`, `\\r`, `\\s`, \
                             `\\\"`, `\\'`, `\\\\`, octal escapes like `\\0` \
                             and unicode escapes like `\\u0041`";

#[derive(Clone, Copy)]
enum Quote {
    Char,
    Str,
    TextBlock,
}

impl Quote {
    fn delimiter_len(self) -> u32 {
        match self {
            Quote::Char | Quote::Str => 1,
            Quote::TextBlock => 3,
        }
    }

    fn mode(self) -> Mode {
        match self {
            Quote::Char => Mode::Char,
            Quote::Str => Mode::Str,
            Quote::TextBlock => Mode::TextBlock,
        }
    }

    fn lit_kind(self) -> LitKind {
        match self {
            Quote::Char => LitKind::Char,
            Quote::Str => LitKind::Str,
            Quote::TextBlock => LitKind::TextBlock,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Quote::Char => "character literal",
            Quote::Str => "string literal",
            Quote::TextBlock => "text block",
        }
    }
}

impl StringReader<'_> {
    pub(super) fn cook_literal(&self, start: BytePos, kind: LiteralKind, suffix_start: u32) -> Lit {
        let end = start + suffix_start;
        let suffix = (end < self.pos).then(|| Symbol::intern(self.str_from(end)));
        let number = || Symbol::intern(self.str_from_to(start, end));

        let (kind, symbol) = match kind {
            LiteralKind::Int { base, empty_int } => {
                (self.cook_int(start, end, base, empty_int, suffix), number())
            }
            LiteralKind::Float { empty_exponent } => (
                self.cook_float(start, end, empty_exponent, suffix),
                number(),
            ),
            LiteralKind::Char { terminated } => {
                self.cook_quoted(Quote::Char, start, end, terminated, suffix)
            }
            LiteralKind::Str { terminated } => {
                self.cook_quoted(Quote::Str, start, end, terminated, suffix)
            }
            LiteralKind::TextBlock { terminated } => {
                self.cook_quoted(Quote::TextBlock, start, end, terminated, suffix)
            }
        };

        Lit::new(kind, symbol, suffix)
    }

    fn cook_int(
        &self,
        start: BytePos,
        end: BytePos,
        base: Base,
        empty_int: bool,
        suffix: Option<Symbol>,
    ) -> LitKind {
        if empty_int {
            self.dcx.emit(
                Diagnostic::error("no valid digits found for number")
                    .with_span(Span::new(start, end)),
            );
            return LitKind::Err;
        }

        let kind = match suffix.map(Symbol::as_str) {
            None | Some("L" | "l") => LitKind::Integer,
            Some("f" | "F" | "d" | "D") if base == Base::Decimal => LitKind::Float,
            Some(suffix) => {
                self.dcx.emit(
                    Diagnostic::error(format!("invalid suffix `{suffix}` for number literal"))
                        .with_span(Span::new(end, self.pos))
                        .with_help(
                            "valid suffixes are `L` for `long`, \
                             and `f` or `d` for decimal floating point numbers",
                        ),
                );
                return LitKind::Err;
            }
        };

        let digits = self.str_from_to(start, end);
        match base {
            Base::Binary => self.check_binary_digits(start, digits).unwrap_or(kind),
            Base::Decimal => self.check_leading_zeros(start, end, digits).unwrap_or(kind),
            Base::Hexadecimal => kind,
        }
    }

    fn check_binary_digits(&self, start: BytePos, digits: &str) -> Option<LitKind> {
        let (offset, digit) = digits
            .char_indices()
            .skip(2)
            .find(|&(_, c)| !matches!(c, '0' | '1' | '_'))?;
        let lo = start + offset as u32;

        self.dcx.emit(
            Diagnostic::error(format!("invalid digit `{digit}` for a base 2 literal"))
                .with_span(Span::new(lo, lo + 1)),
        );
        Some(LitKind::Err)
    }

    fn check_leading_zeros(&self, start: BytePos, end: BytePos, digits: &str) -> Option<LitKind> {
        let has_leading_zero =
            digits.starts_with('0') && digits.bytes().filter(u8::is_ascii_digit).count() > 1;
        if !has_leading_zero {
            return None;
        }

        self.dcx.emit(
            Diagnostic::error("integer literals cannot have leading zeros")
                .with_span(Span::new(start, end))
                .with_note("octal literals are not supported")
                .with_help("remove the leading zeros, or use `0x` for hexadecimal"),
        );
        Some(LitKind::Err)
    }

    fn cook_float(
        &self,
        start: BytePos,
        end: BytePos,
        empty_exponent: bool,
        suffix: Option<Symbol>,
    ) -> LitKind {
        if empty_exponent {
            self.dcx.emit(
                Diagnostic::error("expected at least one digit in exponent")
                    .with_span(Span::new(start, end)),
            );
            return LitKind::Err;
        }

        match suffix.map(Symbol::as_str) {
            None | Some("f" | "F" | "d" | "D") => LitKind::Float,
            Some(suffix) => {
                self.dcx.emit(
                    Diagnostic::error(format!("invalid suffix `{suffix}` for float literal"))
                        .with_span(Span::new(end, self.pos))
                        .with_help("valid suffixes are `f` for `float` and `d` for `double`"),
                );
                LitKind::Err
            }
        }
    }

    fn cook_quoted(
        &self,
        quote: Quote,
        start: BytePos,
        end: BytePos,
        terminated: bool,
        suffix: Option<Symbol>,
    ) -> (LitKind, Symbol) {
        let delimiter = quote.delimiter_len();
        let content_lo = start + delimiter;

        if !terminated {
            self.report_unterminated(quote, start, end);
            return (
                LitKind::Err,
                Symbol::intern(self.str_from_to(content_lo, end)),
            );
        }

        let content_hi = end - delimiter;
        let content = self.str_from_to(content_lo, content_hi);
        let mut kind = quote.lit_kind();

        if suffix.is_some() {
            self.dcx.emit(
                Diagnostic::error(format!("{} cannot have a suffix", quote.description()))
                    .with_span(Span::new(end, self.pos)),
            );
            kind = LitKind::Err;
        }

        if matches!(quote, Quote::TextBlock) && !self.check_text_block_opening(start, content) {
            kind = LitKind::Err;
        }

        unescape::unescape(content, quote.mode(), |range, result| {
            if let Err(error) = result {
                self.report_escape_error(content, content_lo, range, error);
                kind = LitKind::Err;
            }
        });

        (kind, Symbol::intern(content))
    }

    fn report_unterminated(&self, quote: Quote, start: BytePos, end: BytePos) {
        let (message, help) = match quote {
            Quote::Char => ("unterminated character literal", "add a closing `'`"),
            Quote::Str => ("unterminated double quote string", "add a closing `\"`"),
            Quote::TextBlock => ("unterminated text block", "add a closing `\"\"\"`"),
        };
        let span = match quote {
            Quote::TextBlock => Span::new(start, start + 3),
            Quote::Char | Quote::Str => Span::new(start, end),
        };
        self.dcx
            .emit(Diagnostic::error(message).with_span(span).with_help(help));
    }

    fn check_text_block_opening(&self, start: BytePos, content: &str) -> bool {
        let opening_line = content.split_once('\n').map(|(line, _)| line);
        if opening_line.is_some_and(|line| line.trim().is_empty()) {
            return true;
        }

        self.dcx.emit(
            Diagnostic::error("text block opening delimiter must be followed by a line break")
                .with_span(Span::new(start, start + 3))
                .with_help("start the content of the text block on the next line"),
        );
        false
    }

    fn report_escape_error(
        &self,
        content: &str,
        content_lo: BytePos,
        range: Range<usize>,
        error: EscapeError,
    ) {
        let span = Span::new(content_lo, content_lo).subspan(range.clone());
        let escaped = &content[range];

        let diagnostic = match error {
            EscapeError::ZeroChars => Diagnostic::error("empty character literal")
                .with_span(Span::new(content_lo - 1, content_lo + 1)),
            EscapeError::MoreThanOneChar => {
                Diagnostic::error("character literal may only contain one codepoint")
                    .with_span(span)
                    .with_help("if you meant to write a string literal, use double quotes")
            }
            EscapeError::LoneSlash => {
                Diagnostic::error("invalid trailing slash in literal").with_span(span)
            }
            EscapeError::InvalidEscape => Diagnostic::error(format!(
                "unknown character escape: `{}`",
                escaped[1..].escape_debug()
            ))
            .with_span(span)
            .with_help(VALID_ESCAPES),
            EscapeError::EscapeOnlyChar => {
                Diagnostic::error("character literal `'` must be escaped")
                    .with_span(span)
                    .with_help("escape the character: `\\'`")
            }
            EscapeError::InvalidUnicodeEscape => Diagnostic::error("invalid unicode escape")
                .with_span(span)
                .with_help("unicode escapes need exactly four hexadecimal digits, like `\\u0041`"),
            EscapeError::SurrogateUnicodeEscape => {
                Diagnostic::error("unicode escape must not be a surrogate")
                    .with_span(span)
                    .with_note("code points from `\\uD800` to `\\uDFFF` are not valid characters")
            }
        };

        self.dcx.emit(diagnostic);
    }
}

use krelpin_ast::ast::{CapturedTokens, Ident};
use krelpin_ast::token::{Token, TokenKind};
use krelpin_errors::Diagnostic;
use krelpin_parse::lexer;
use krelpin_span::{Expansion, Span};

use crate::proc_macro::{CallError, WireKind, WireToken};
use crate::table::MacroKind;
use crate::{ExpandCtxt, Invocation, RECURSION_LIMIT};

impl ExpandCtxt<'_> {
    pub(crate) fn invoke(
        &self,
        invocation: &Invocation,
        captured: &CapturedTokens,
        target: Ident,
        depth: usize,
    ) -> Option<Vec<Token>> {
        let MacroKind::Extern { krate, index, name } = invocation.kind else {
            return None;
        };
        let annotation = &invocation.annotation;
        if depth >= RECURSION_LIMIT {
            self.emit(
                Diagnostic::error(format!("recursion limit reached while expanding `@{name}`"))
                    .with_span(annotation.span)
                    .with_note(format!(
                        "macros were expanded {RECURSION_LIMIT} times inside each other"
                    )),
            );
            return None;
        }
        let extern_crate = &self.externs[krate];
        let qualified = &extern_crate.macros()[index];
        let expansion = Expansion {
            call_site: annotation.span,
            macro_name: name.as_str().to_owned(),
        };

        let mut spans: Vec<Span> = Vec::new();
        let args: Vec<WireToken> = annotation
            .args
            .as_ref()
            .map_or(&[][..], |args| args.tokens.as_slice())
            .iter()
            .map(|token| self.wire(token, &mut spans))
            .collect();
        let item: Vec<WireToken> = self
            .target_tokens(captured, annotation.span)
            .iter()
            .map(|token| self.wire(token, &mut spans))
            .collect();

        let mut lex = |source: &str| {
            let file = self.source_map.new_expansion_file(
                format!("<TokenStream.parse in @{name}>"),
                source.to_owned(),
                expansion.clone(),
            );
            lexer::lex(&file, self.dcx)
                .iter()
                .filter(|token| token.kind != TokenKind::Eof)
                .map(|token| self.wire(token, &mut spans))
                .collect()
        };
        let result = extern_crate.call(qualified, &args, &item, &mut lex);
        match result {
            Ok(output) => {
                let file_name = format!("<@{name} on {}>", target.name);
                self.rebuild(&output, &spans, &file_name, &expansion)
            }
            Err(CallError::Reported { message, span }) => {
                let at = span
                    .and_then(|index| spans.get(index).copied())
                    .unwrap_or(annotation.span);
                let mut diagnostic = Diagnostic::error(message).with_span(at);
                if at != annotation.span {
                    diagnostic =
                        diagnostic.with_secondary(annotation.span, format!("in `@{name}`"));
                }
                self.emit(diagnostic);
                None
            }
            Err(CallError::Panicked(message)) => {
                self.emit(
                    Diagnostic::error(format!("the macro `@{name}` panicked"))
                        .with_span(annotation.span)
                        .with_note(format!("it threw {message}")),
                );
                None
            }
            Err(CallError::Failed(message)) => {
                self.emit(
                    Diagnostic::error(format!("the macro `@{name}` failed"))
                        .with_span(annotation.span)
                        .with_note(format!(
                            "{message}; the macro comes from `{}`",
                            extern_crate.path().display()
                        )),
                );
                None
            }
        }
    }

    fn target_tokens(&self, captured: &CapturedTokens, invoked: Span) -> Vec<Token> {
        let removed = self.config.removed.borrow();
        let inside = |span: Span, region: &Span| span.lo >= region.lo && span.hi <= region.hi;
        let mut out: Vec<Token> = Vec::new();
        for token in captured.tokens().unwrap_or(&[]) {
            if token.kind == TokenKind::Eof || inside(token.span, &invoked) {
                continue;
            }
            if removed
                .members
                .iter()
                .any(|region| inside(token.span, region))
            {
                while out
                    .last()
                    .is_some_and(|last| matches!(last.kind, TokenKind::DocComment(_)))
                {
                    out.pop();
                }
                continue;
            }
            if removed
                .annotations
                .iter()
                .any(|region| inside(token.span, region))
            {
                continue;
            }
            out.push(*token);
        }
        out
    }

    fn wire(&self, token: &Token, spans: &mut Vec<Span>) -> WireToken {
        let kind = match token.kind {
            TokenKind::Ident(_) => WireKind::Ident,
            TokenKind::Literal(_) => WireKind::Literal,
            TokenKind::DocComment(_) => WireKind::Doc,
            _ => WireKind::Punct,
        };
        let text = self
            .source_map
            .span_to_snippet(token.span)
            .or_else(|| token.kind.as_str().map(str::to_owned))
            .unwrap_or_default();
        spans.push(token.span);
        WireToken {
            kind,
            span: Some(spans.len() - 1),
            text,
        }
    }

    fn rebuild(
        &self,
        output: &[WireToken],
        spans: &[Span],
        file_name: &str,
        expansion: &Expansion,
    ) -> Option<Vec<Token>> {
        let mut src = String::new();
        let mut offsets = Vec::with_capacity(output.len());
        for token in output {
            offsets.push(src.len());
            src.push_str(&token.text);
            let breaks =
                token.kind == WireKind::Doc || matches!(token.text.as_str(), ";" | "{" | "}");
            src.push(if breaks { '\n' } else { ' ' });
        }
        let errors = self.dcx.error_count();
        let file = self
            .source_map
            .new_expansion_file(file_name, src, expansion.clone());
        let lexed: Vec<Token> = lexer::lex(&file, self.dcx)
            .into_iter()
            .filter(|token| token.kind != TokenKind::Eof)
            .collect();
        if self.dcx.error_count() > errors {
            return None;
        }
        let mut tokens = Vec::with_capacity(output.len());
        for (index, wire) in output.iter().enumerate() {
            let start = file.start_pos + offsets[index] as u32;
            let expected = Span::new(start, start + wire.text.len() as u32);
            let lexed_token = lexed.get(index).filter(|token| token.span == expected);
            let Some(token) = lexed_token else {
                self.invalid_token(wire, "is not a single token", expansion);
                return None;
            };
            let kind = match token.kind {
                TokenKind::Ident(_) => WireKind::Ident,
                TokenKind::Literal(_) => WireKind::Literal,
                TokenKind::DocComment(_) => WireKind::Doc,
                _ => WireKind::Punct,
            };
            if kind != wire.kind {
                let found = format!(
                    "was made as {}, but it is {}",
                    wire.kind.description(),
                    kind.description()
                );
                self.invalid_token(wire, &found, expansion);
                return None;
            }
            let span = wire
                .span
                .and_then(|index| spans.get(index).copied())
                .filter(|span| {
                    self.source_map.span_to_snippet(*span).as_deref() == Some(&wire.text)
                })
                .unwrap_or(token.span);
            tokens.push(Token::new(token.kind, span));
        }
        if lexed.len() != output.len() {
            self.emit(
                Diagnostic::error("a macro produced text that is not a list of tokens")
                    .with_span(expansion.call_site),
            );
            return None;
        }
        Some(tokens)
    }

    fn invalid_token(&self, token: &WireToken, problem: &str, expansion: &Expansion) {
        self.emit(
            Diagnostic::error(format!(
                "the macro `@{}` produced an invalid token",
                expansion.macro_name
            ))
            .with_span(expansion.call_site)
            .with_note(format!("`{}` {problem}", token.text)),
        );
    }
}

use std::io::{self, Write};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use krelpin_span::{Loc, SourceMap, Span};

use crate::{Diagnostic, Level};

const TAB_WIDTH: usize = 4;
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const GUTTER: &str = "\x1b[1;34m";

pub trait Emitter {
    fn emit(&mut self, diagnostic: &Diagnostic);
}

#[derive(Clone, Default)]
pub struct DiagnosticBuffer(Arc<Mutex<Vec<Diagnostic>>>);

impl DiagnosticBuffer {
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.lock().clone()
    }

    pub fn messages(&self) -> Vec<String> {
        self.lock()
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect()
    }
}

impl DiagnosticBuffer {
    fn lock(&self) -> MutexGuard<'_, Vec<Diagnostic>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Emitter for DiagnosticBuffer {
    fn emit(&mut self, diagnostic: &Diagnostic) {
        self.lock().push(diagnostic.clone());
    }
}

pub struct HumanEmitter<W> {
    source_map: Arc<SourceMap>,
    writer: W,
    color: bool,
}

impl HumanEmitter<io::Stderr> {
    pub fn stderr(source_map: Arc<SourceMap>, color: bool) -> Self {
        HumanEmitter::new(source_map, io::stderr(), color)
    }
}

impl<W: Write> HumanEmitter<W> {
    pub fn new(source_map: Arc<SourceMap>, writer: W, color: bool) -> Self {
        HumanEmitter {
            source_map,
            writer,
            color,
        }
    }

    pub fn render(&self, diagnostic: &Diagnostic) -> String {
        let level = diagnostic.level;
        let mut lines = vec![format!(
            "{}{}",
            self.paint(level_color(level), level.as_str()),
            self.paint(BOLD, &format!(": {}", diagnostic.message)),
        )];

        let location = diagnostic
            .span
            .and_then(|span| Some((span, self.source_map.lookup_char_pos(span.lo)?)));
        let width = location.as_ref().map_or(1, |(_, loc)| digits(loc.line));
        let pad = " ".repeat(width);
        let bar = self.paint(GUTTER, "|");

        if let Some((span, loc)) = &location {
            self.render_snippet(&mut lines, diagnostic, *span, loc, &pad);
        }

        if !diagnostic.children.is_empty() {
            if location.is_some() {
                lines.push(format!("{pad} {bar}"));
            }
            for child in &diagnostic.children {
                lines.push(format!(
                    "{pad} {} {}: {}",
                    self.paint(GUTTER, "="),
                    self.paint(BOLD, child.level.as_str()),
                    child.message,
                ));
            }
        }

        lines.join("\n") + "\n\n"
    }

    fn render_snippet(
        &self,
        lines: &mut Vec<String>,
        diagnostic: &Diagnostic,
        span: Span,
        loc: &Loc,
        pad: &str,
    ) {
        let bar = self.paint(GUTTER, "|");
        let text = loc.file.line(loc.line - 1);
        let start = text
            .char_indices()
            .nth(loc.col - 1)
            .map_or(text.len(), |(index, _)| index);
        let end = (start + span.len() as usize).min(text.len());
        let end = (end..=text.len())
            .find(|&index| text.is_char_boundary(index))
            .unwrap_or(text.len());

        let indent = display_width(&text[..start]);
        let carets = "^".repeat(display_width(&text[start..end]).max(1));
        let label = diagnostic
            .label
            .as_ref()
            .map_or(String::new(), |label| format!(" {label}"));
        let marker = self.paint(level_color(diagnostic.level), &(carets + &label));

        lines.push(format!(
            "{pad}{} {}:{}:{}",
            self.paint(GUTTER, "-->"),
            loc.file.name.display(),
            loc.line,
            loc.col,
        ));
        lines.push(format!("{pad} {bar}"));
        lines.push(format!(
            "{} {bar} {}",
            self.paint(GUTTER, &loc.line.to_string()),
            expand_tabs(text),
        ));
        lines.push(format!("{pad} {bar} {}{marker}", " ".repeat(indent)));
    }

    fn paint(&self, style: &str, text: &str) -> String {
        if self.color {
            format!("{style}{text}{RESET}")
        } else {
            text.to_owned()
        }
    }
}

impl<W: Write> Emitter for HumanEmitter<W> {
    fn emit(&mut self, diagnostic: &Diagnostic) {
        let rendered = self.render(diagnostic);
        let _ = self.writer.write_all(rendered.as_bytes());
        let _ = self.writer.flush();
    }
}

fn level_color(level: Level) -> &'static str {
    match level {
        Level::Error => "\x1b[1;31m",
        Level::Warning => "\x1b[1;33m",
        Level::Note => "\x1b[1;32m",
        Level::Help => "\x1b[1;36m",
    }
}

fn digits(number: usize) -> usize {
    number.to_string().len()
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| if c == '\t' { TAB_WIDTH } else { 1 })
        .sum()
}

fn expand_tabs(text: &str) -> String {
    text.replace('\t', &" ".repeat(TAB_WIDTH))
}

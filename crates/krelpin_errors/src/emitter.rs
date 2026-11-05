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

        let mut origins = Vec::new();
        let mut annotations = Vec::new();
        if let Some(span) = diagnostic.span {
            let span = self.call_site(span, &mut origins);
            if let Some(loc) = self.source_map.lookup_char_pos(span.lo) {
                annotations.push(Annotation {
                    span,
                    loc,
                    label: diagnostic.label.clone(),
                    primary: true,
                });
            }
        }
        for secondary in &diagnostic.secondary {
            let span = self.call_site(secondary.span, &mut Vec::new());
            let Some(loc) = self.source_map.lookup_char_pos(span.lo) else {
                continue;
            };
            let same_file = annotations
                .first()
                .is_some_and(|primary| Arc::ptr_eq(&primary.loc.file, &loc.file));
            if same_file {
                annotations.push(Annotation {
                    span,
                    loc,
                    label: Some(secondary.label.clone()),
                    primary: false,
                });
            }
        }
        let width = annotations
            .iter()
            .map(|annotation| digits(annotation.loc.line))
            .max()
            .unwrap_or(1);
        let pad = " ".repeat(width);
        let bar = self.paint(GUTTER, "|");

        let has_snippet = !annotations.is_empty();
        if has_snippet {
            self.render_snippet(&mut lines, diagnostic.level, annotations, &pad);
        }

        let children = diagnostic
            .children
            .iter()
            .map(|child| (child.level.as_str(), child.message.clone()))
            .chain(collapse(origins).into_iter().map(|(name, count)| {
                let level = Level::Note.as_str();
                let message = if count == 1 {
                    format!("this comes from the expansion of `@{name}`")
                } else {
                    format!("this comes from {count} nested expansions of `@{name}`")
                };
                (level, message)
            }))
            .collect::<Vec<_>>();
        if !children.is_empty() {
            if has_snippet {
                lines.push(format!("{pad} {bar}"));
            }
            for (level, message) in children {
                lines.push(format!(
                    "{pad} {} {}: {message}",
                    self.paint(GUTTER, "="),
                    self.paint(BOLD, level),
                ));
            }
        }

        lines.join("\n") + "\n\n"
    }

    fn render_snippet(
        &self,
        lines: &mut Vec<String>,
        level: Level,
        mut annotations: Vec<Annotation>,
        pad: &str,
    ) {
        let bar = self.paint(GUTTER, "|");
        let primary = &annotations[0];
        lines.push(format!(
            "{pad}{} {}:{}:{}",
            self.paint(GUTTER, "-->"),
            primary.loc.file.name.display(),
            primary.loc.line,
            primary.loc.col,
        ));
        lines.push(format!("{pad} {bar}"));

        annotations.sort_by_key(|annotation| (annotation.loc.line, annotation.loc.col));
        let mut previous: Option<usize> = None;
        let mut index = 0;
        while index < annotations.len() {
            let line = annotations[index].loc.line;
            if previous.is_some_and(|previous| line > previous + 1) {
                lines.push(self.paint(GUTTER, "..."));
            }
            let file = Arc::clone(&annotations[index].loc.file);
            let text = file.line(line - 1);
            let number = format!("{line:>width$}", width = pad.len());
            lines.push(format!(
                "{} {bar} {}",
                self.paint(GUTTER, &number),
                expand_tabs(text),
            ));
            while index < annotations.len() && annotations[index].loc.line == line {
                let annotation = &annotations[index];
                let start = text
                    .char_indices()
                    .nth(annotation.loc.col - 1)
                    .map_or(text.len(), |(index, _)| index);
                let end = (start + annotation.span.len() as usize).min(text.len());
                let end = (end..=text.len())
                    .find(|&index| text.is_char_boundary(index))
                    .unwrap_or(text.len());
                let indent = display_width(&text[..start]);
                let (mark, color) = if annotation.primary {
                    ("^", level_color(level))
                } else {
                    ("-", GUTTER)
                };
                let marks = mark.repeat(display_width(&text[start..end]).max(1));
                let label = annotation
                    .label
                    .as_ref()
                    .map_or(String::new(), |label| format!(" {label}"));
                let marker = self.paint(color, &(marks + &label));
                lines.push(format!("{pad} {bar} {}{marker}", " ".repeat(indent)));
                index += 1;
            }
            previous = Some(line);
        }
    }

    fn call_site(&self, mut span: Span, origins: &mut Vec<String>) -> Span {
        while let Some(expansion) = self.source_map.expansion_of(span) {
            origins.push(expansion.macro_name);
            span = expansion.call_site;
        }
        span
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

struct Annotation {
    span: Span,
    loc: Loc,
    label: Option<String>,
    primary: bool,
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

fn collapse(origins: Vec<String>) -> Vec<(String, usize)> {
    let mut runs: Vec<(String, usize)> = Vec::new();
    for name in origins {
        match runs.last_mut() {
            Some((last, count)) if *last == name => *count += 1,
            _ => runs.push((name, 1)),
        }
    }
    runs
}

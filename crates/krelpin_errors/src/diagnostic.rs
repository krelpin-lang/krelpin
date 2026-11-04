use std::fmt;

use krelpin_span::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Note,
    Help,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Note => "note",
            Level::Help => "help",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubDiagnostic {
    pub level: Level,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanLabel {
    pub span: Span,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub level: Level,
    pub message: String,
    pub span: Option<Span>,
    pub label: Option<String>,
    pub secondary: Vec<SpanLabel>,
    pub children: Vec<SubDiagnostic>,
    pub lint: Option<&'static str>,
}

impl Diagnostic {
    pub fn new(level: Level, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            level,
            message: message.into(),
            span: None,
            label: None,
            secondary: Vec::new(),
            children: Vec::new(),
            lint: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Level::Error, message)
    }

    pub fn warning(message: impl Into<String>) -> Diagnostic {
        Diagnostic::new(Level::Warning, message)
    }

    pub fn lint(lint: &'static str, message: impl Into<String>) -> Diagnostic {
        Diagnostic {
            lint: Some(lint),
            ..Diagnostic::new(Level::Warning, message)
        }
    }

    pub fn with_span(mut self, span: Span) -> Diagnostic {
        self.span = Some(span);
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Diagnostic {
        self.label = Some(label.into());
        self
    }

    pub fn with_secondary(mut self, span: Span, label: impl Into<String>) -> Diagnostic {
        self.secondary.push(SpanLabel {
            span,
            label: label.into(),
        });
        self
    }

    pub fn with_note(self, message: impl Into<String>) -> Diagnostic {
        self.with_child(Level::Note, message)
    }

    pub fn with_help(self, message: impl Into<String>) -> Diagnostic {
        self.with_child(Level::Help, message)
    }

    fn with_child(mut self, level: Level, message: impl Into<String>) -> Diagnostic {
        self.children.push(SubDiagnostic {
            level,
            message: message.into(),
        });
        self
    }
}

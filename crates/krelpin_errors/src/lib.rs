mod diagnostic;
mod emitter;
pub mod lint;

#[cfg(test)]
mod tests;

use std::sync::{Mutex, MutexGuard, PoisonError};

pub use crate::diagnostic::{Diagnostic, Level, SpanLabel, SubDiagnostic};
pub use crate::emitter::{DiagnosticBuffer, Emitter, HumanEmitter};
pub use crate::lint::{LintLevel, LintLevels};

pub struct DiagCtxt {
    inner: Mutex<DiagCtxtInner>,
}

struct DiagCtxtInner {
    emitter: Box<dyn Emitter + Send>,
    error_count: usize,
    warning_count: usize,
    lints: LintLevels,
}

impl DiagCtxt {
    pub fn new(emitter: impl Emitter + Send + 'static) -> DiagCtxt {
        DiagCtxt {
            inner: Mutex::new(DiagCtxtInner {
                emitter: Box::new(emitter),
                error_count: 0,
                warning_count: 0,
                lints: LintLevels::default(),
            }),
        }
    }

    pub fn set_lint_levels(&self, lints: LintLevels) {
        self.lock().lints = lints;
    }

    pub fn emit(&self, mut diagnostic: Diagnostic) {
        let mut inner = self.lock();
        if let Some(name) = diagnostic.lint {
            let (level, source) = inner.lints.level(name);
            let note = match (level, source) {
                (LintLevel::Allow, _) => return,
                (_, Some(source)) if source.starts_with("--") => format!("`{source}` is set"),
                (level, Some(source)) => format!("`{} {source}` is set", level.flag()),
                (_, None) => format!("`{name}` is a warning by default; allow it with `-A {name}`"),
            };
            if level >= LintLevel::Deny {
                diagnostic.level = Level::Error;
            }
            diagnostic = diagnostic.with_note(note);
        }
        match diagnostic.level {
            Level::Error => inner.error_count += 1,
            Level::Warning => inner.warning_count += 1,
            Level::Note | Level::Help => {}
        }
        inner.emitter.emit(&diagnostic);
    }

    pub fn error_count(&self) -> usize {
        self.lock().error_count
    }

    pub fn warning_count(&self) -> usize {
        self.lock().warning_count
    }

    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }

    fn lock(&self) -> MutexGuard<'_, DiagCtxtInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

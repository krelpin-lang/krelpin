mod diagnostic;
mod emitter;

#[cfg(test)]
mod tests;

use std::sync::{Mutex, MutexGuard, PoisonError};

pub use crate::diagnostic::{Diagnostic, Level, SpanLabel, SubDiagnostic};
pub use crate::emitter::{DiagnosticBuffer, Emitter, HumanEmitter};

pub struct DiagCtxt {
    inner: Mutex<DiagCtxtInner>,
}

struct DiagCtxtInner {
    emitter: Box<dyn Emitter + Send>,
    error_count: usize,
    warning_count: usize,
}

impl DiagCtxt {
    pub fn new(emitter: impl Emitter + Send + 'static) -> DiagCtxt {
        DiagCtxt {
            inner: Mutex::new(DiagCtxtInner {
                emitter: Box::new(emitter),
                error_count: 0,
                warning_count: 0,
            }),
        }
    }

    pub fn emit(&self, diagnostic: Diagnostic) {
        let mut inner = self.lock();
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

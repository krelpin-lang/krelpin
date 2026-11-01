mod diagnostic;
mod emitter;

#[cfg(test)]
mod tests;

use std::cell::RefCell;

pub use crate::diagnostic::{Diagnostic, Level, SubDiagnostic};
pub use crate::emitter::{DiagnosticBuffer, Emitter, HumanEmitter};

pub struct DiagCtxt {
    inner: RefCell<DiagCtxtInner>,
}

struct DiagCtxtInner {
    emitter: Box<dyn Emitter>,
    error_count: usize,
    warning_count: usize,
}

impl DiagCtxt {
    pub fn new(emitter: impl Emitter + 'static) -> DiagCtxt {
        DiagCtxt {
            inner: RefCell::new(DiagCtxtInner {
                emitter: Box::new(emitter),
                error_count: 0,
                warning_count: 0,
            }),
        }
    }

    pub fn emit(&self, diagnostic: Diagnostic) {
        let mut inner = self.inner.borrow_mut();
        match diagnostic.level {
            Level::Error => inner.error_count += 1,
            Level::Warning => inner.warning_count += 1,
            Level::Note | Level::Help => {}
        }
        inner.emitter.emit(&diagnostic);
    }

    pub fn error_count(&self) -> usize {
        self.inner.borrow().error_count
    }

    pub fn warning_count(&self) -> usize {
        self.inner.borrow().warning_count
    }

    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }
}

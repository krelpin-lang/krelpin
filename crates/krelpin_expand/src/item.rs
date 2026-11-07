use krelpin_errors::Diagnostic;

use crate::{ExpandCtxt, Invocation};

impl ExpandCtxt<'_> {
    pub(crate) fn misplaced(&self, invocation: &Invocation, target: &str) {
        self.emit(
            Diagnostic::error(format!(
                "`@{}` cannot be used on {target}",
                invocation.kind.name()
            ))
            .with_span(invocation.annotation.span)
            .with_help(invocation.usage()),
        );
    }
}

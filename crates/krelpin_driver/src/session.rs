use std::io::IsTerminal;
use std::process::ExitCode;
use std::rc::Rc;

use krelpin_errors::{DiagCtxt, Diagnostic, HumanEmitter};
use krelpin_span::SourceMap;

pub struct Session {
    pub source_map: Rc<SourceMap>,
    pub dcx: DiagCtxt,
}

impl Session {
    pub fn new() -> Session {
        let source_map = Rc::new(SourceMap::new());
        let color = std::io::stderr().is_terminal();
        let emitter = HumanEmitter::stderr(Rc::clone(&source_map), color);
        Session {
            source_map,
            dcx: DiagCtxt::new(emitter),
        }
    }

    pub fn finish(&self) -> ExitCode {
        let count = self.dcx.error_count();
        if count == 0 {
            return ExitCode::SUCCESS;
        }

        let plural = if count == 1 { "" } else { "s" };
        self.dcx.emit(Diagnostic::error(format!(
            "aborting due to {count} previous error{plural}"
        )));
        ExitCode::FAILURE
    }
}

impl Default for Session {
    fn default() -> Session {
        Session::new()
    }
}

mod args;
mod session;

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use krelpin_ast::token::Token;
use krelpin_errors::Diagnostic;
use krelpin_sysroot::Sysroot;

pub use crate::args::{Args, CrateType, Edition, Emit, LintLevel, PrintKind, Spec};
pub use crate::session::Session;

pub fn main() -> ExitCode {
    let args = Args::parse();
    let sess = Session::new();
    run(&args, &sess);
    sess.finish()
}

pub fn run(args: &Args, sess: &Session) {
    if let Some(code) = &args.explain {
        println!("no extended information for {code}");
        return;
    }

    if !args.print.is_empty() {
        for request in &args.print {
            print_info(args, sess, request);
        }
        return;
    }

    let Some(input) = &args.input else {
        return;
    };

    let file = match sess.source_map.load_file(input) {
        Ok(file) => file,
        Err(error) => {
            sess.dcx.emit(Diagnostic::error(format!(
                "couldn't read `{}`: {error}",
                input.display()
            )));
            return;
        }
    };

    if args.verbose {
        eprintln!(
            "compiling `{}` (opt-level={}, debuginfo={})",
            args.crate_name(),
            args.opt_level(),
            args.debuginfo_level()
        );
    }

    let emits = args.emits();
    let tokens = krelpin_parse::lexer::lex(&file, &sess.dcx);
    if sess.dcx.has_errors() {
        return;
    }

    for emit in emits.iter().filter(|emit| emit.kind == Emit::Tokens) {
        write_output(sess, emit.path.as_deref(), &render_tokens(sess, &tokens));
    }
    if emits.iter().all(|emit| emit.kind == Emit::Tokens) {
        return;
    }

    let unit = krelpin_parse::parser::parse(tokens, &sess.dcx);
    if sess.dcx.has_errors() {
        return;
    }

    for emit in emits.iter().filter(|emit| emit.kind == Emit::Ast) {
        write_output(sess, emit.path.as_deref(), &format!("{unit:#?}\n"));
    }
    let needs_analysis = |kind: Emit| !matches!(kind, Emit::Tokens | Emit::Ast);
    if !emits.iter().any(|emit| needs_analysis(emit.kind)) {
        return;
    }

    let Some(sysroot) = locate_sysroot(args, sess) else {
        return;
    };
    let mut units = sysroot.load_std(&sess.source_map, &sess.dcx);
    if sess.dcx.has_errors() {
        return;
    }
    units.push(unit);
    krelpin_expand::expand(&mut units, &sess.source_map, &sess.dcx);
    if sess.dcx.has_errors() {
        return;
    }
    let resolved = krelpin_resolve::resolve(&units, &sess.dcx);
    if sess.dcx.has_errors() {
        return;
    }

    let typeck = krelpin_typeck::check(&units, &resolved, &sess.dcx);
    if sess.dcx.has_errors() {
        return;
    }

    let hir = krelpin_hir::lower(&units, &resolved, &typeck.tcx, &typeck.results);
    let user_unit = units.len() - 1;
    for emit in emits.iter().filter(|emit| emit.kind == Emit::Hir) {
        let bodies = hir
            .bodies
            .iter()
            .filter(|body| resolved.items.def(body.owner).unit == user_unit);
        let rendered = krelpin_hir::print(bodies, &typeck.tcx);
        write_output(sess, emit.path.as_deref(), &rendered);
    }

    let mir = krelpin_mir::build(&hir, &typeck.tcx);
    let all_bodies = (0..mir.bodies.len()).map(|index| krelpin_mir::BodyId(index as u32));
    for emit in emits.iter().filter(|emit| emit.kind == Emit::Mir) {
        let bodies = all_bodies
            .clone()
            .filter(|&id| resolved.items.def(mir.body(id).owner).unit == user_unit);
        let rendered = krelpin_mir::print(&mir, bodies, &typeck.tcx);
        write_output(sess, emit.path.as_deref(), &rendered);
    }
    krelpin_borrowck::check(&mir, all_bodies, &typeck.tcx, &sess.dcx);
    if sess.dcx.has_errors() {
        return;
    }

    let wants_code = |kind: Emit| matches!(kind, Emit::Ir | Emit::Asm | Emit::Obj | Emit::Link);
    if !emits.iter().any(|emit| wants_code(emit.kind)) {
        return;
    }
    let entry = krelpin_codegen_llvm::find_entry(&mir, &typeck.tcx, |def| {
        resolved.items.def(def).unit == user_unit
    });
    let entry = match entry {
        Ok(entry) => entry,
        Err(diagnostic) => {
            sess.dcx.emit(diagnostic);
            return;
        }
    };
    let Some(ir) = krelpin_codegen_llvm::codegen(&mir, &typeck.tcx, entry, &sess.dcx) else {
        return;
    };
    let runtime_dir = sysroot.root().join("library/runtime");
    for emit in emits.iter().filter(|emit| wants_code(emit.kind)) {
        let path = output_path(args, emit);
        if emit.kind == Emit::Ir {
            write_output(sess, Some(&path), &ir);
            continue;
        }
        let kind = match emit.kind {
            Emit::Asm => krelpin_codegen_llvm::OutputKind::Asm,
            Emit::Obj => krelpin_codegen_llvm::OutputKind::Object,
            _ => krelpin_codegen_llvm::OutputKind::Executable,
        };
        let options = krelpin_codegen_llvm::LinkOptions {
            runtime_dir: &runtime_dir,
            opt_level: args.opt_level(),
            debuginfo: args.debuginfo_level() > 0,
            output: &path,
            kind,
        };
        if let Err(message) = krelpin_codegen_llvm::link(&ir, &options) {
            sess.dcx
                .emit(Diagnostic::error(format!("linking failed: {message}")));
        }
    }
}

fn output_path(args: &Args, emit: &Spec<Emit>) -> std::path::PathBuf {
    if let Some(path) = &emit.path {
        return path.clone();
    }
    if let Some(path) = &args.output {
        return path.clone();
    }
    let extension = match emit.kind {
        Emit::Ir => "ll",
        Emit::Asm => "s",
        Emit::Obj => "o",
        _ => "",
    };
    let mut name = args.crate_name();
    if !extension.is_empty() {
        name = format!("{name}.{extension}");
    }
    match &args.out_dir {
        Some(dir) => dir.join(name),
        None => std::path::PathBuf::from(name),
    }
}

fn locate_sysroot(args: &Args, sess: &Session) -> Option<Sysroot> {
    match Sysroot::locate(args.sysroot.as_deref()) {
        Ok(sysroot) => Some(sysroot),
        Err(error) => {
            sess.dcx.emit(error.into_diagnostic());
            None
        }
    }
}

fn render_tokens(sess: &Session, tokens: &[Token]) -> String {
    tokens
        .iter()
        .map(|token| format!("{}: {:?}\n", location(sess, token), token.kind))
        .collect()
}

fn location(sess: &Session, token: &Token) -> String {
    match sess.source_map.lookup_char_pos(token.span.lo) {
        Some(loc) => format!("{}:{}", loc.line, loc.col),
        None => "?".to_owned(),
    }
}

fn print_info(args: &Args, sess: &Session, request: &Spec<PrintKind>) {
    let output = match request.kind {
        PrintKind::CrateName => args.crate_name(),
        PrintKind::Cfg => args.cfg.join("\n"),
        PrintKind::HostTuple => {
            format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
        }
        PrintKind::Sysroot => match locate_sysroot(args, sess) {
            Some(sysroot) => sysroot.root().display().to_string(),
            None => return,
        },
        PrintKind::SupportedCrateTypes => CrateType::value_variants()
            .iter()
            .map(|&kind| value_name(kind))
            .collect::<Vec<_>>()
            .join("\n"),
        kind => {
            sess.dcx.emit(Diagnostic::error(format!(
                "`--print={}` is not supported yet",
                value_name(kind)
            )));
            return;
        }
    };

    write_output(sess, request.path.as_deref(), &format!("{output}\n"));
}

fn write_output(sess: &Session, path: Option<&Path>, content: &str) {
    let Some(path) = path else {
        print!("{content}");
        return;
    };

    if let Err(error) = fs::write(path, content) {
        sess.dcx.emit(Diagnostic::error(format!(
            "couldn't write `{}`: {error}",
            path.display()
        )));
    }
}

fn value_name<T: ValueEnum>(value: T) -> String {
    value
        .to_possible_value()
        .map_or_else(String::new, |value| value.get_name().to_owned())
}

#[cfg(test)]
mod tests {
    use super::Session;

    #[test]
    fn session_is_thread_safe() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Session>();
    }
}

mod args;
mod session;

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use krelpin_ast::token::Token;
use krelpin_errors::Diagnostic;

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

    let resolved = krelpin_resolve::resolve(&unit, &sess.dcx);
    if sess.dcx.has_errors() {
        return;
    }

    krelpin_typeck::check(&unit, &resolved.resolutions, &sess.dcx);
    if sess.dcx.has_errors() {
        return;
    }

    for emit in &emits {
        match emit.kind {
            Emit::Tokens | Emit::Ast | Emit::Metadata => {}
            kind => sess.dcx.emit(Diagnostic::error(format!(
                "`--emit={}` is not supported yet",
                value_name(kind)
            ))),
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

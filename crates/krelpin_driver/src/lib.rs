mod args;

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};

pub use crate::args::{Args, CrateType, Edition, Emit, LintLevel, PrintKind, Spec};

pub fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

pub fn run(args: &Args) -> Result<(), String> {
    if let Some(code) = &args.explain {
        println!("no extended information for {code}");
        return Ok(());
    }

    if !args.print.is_empty() {
        return args
            .print
            .iter()
            .try_for_each(|request| print_info(args, request));
    }

    let Some(input) = &args.input else {
        return Ok(());
    };

    let _src = fs::read_to_string(input)
        .map_err(|error| format!("couldn't read `{}`: {error}", input.display()))?;

    if args.verbose {
        eprintln!(
            "compiling `{}` (opt-level={}, debuginfo={})",
            args.crate_name(),
            args.opt_level(),
            args.debuginfo_level()
        );
    }

    match args.emits().into_iter().next() {
        Some(emit) => Err(format!(
            "`--emit={}` is not supported yet",
            value_name(emit.kind)
        )),
        None => Ok(()),
    }
}

fn print_info(args: &Args, request: &Spec<PrintKind>) -> Result<(), String> {
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
            return Err(format!(
                "`--print={}` is not supported yet",
                value_name(kind)
            ));
        }
    };

    write_output(request.path.as_deref(), &format!("{output}\n"))
}

fn write_output(path: Option<&Path>, content: &str) -> Result<(), String> {
    let Some(path) = path else {
        print!("{content}");
        return Ok(());
    };

    fs::write(path, content)
        .map_err(|error| format!("couldn't write `{}`: {error}", path.display()))
}

fn value_name<T: ValueEnum>(value: T) -> String {
    value
        .to_possible_value()
        .map_or_else(String::new, |value| value.get_name().to_owned())
}

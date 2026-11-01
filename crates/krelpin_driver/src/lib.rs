mod args;

pub use args::{Args, CrateType, Edition, Emit, LintLevel, PrintKind, Spec};

use clap::{Parser, ValueEnum};
use std::process::ExitCode;

pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

pub fn run(args: &Args) -> Result {
    if let Some(code) = &args.explain {
        println!("no extended information for {code}");
        return Ok(());
    }

    if !args.print.is_empty() {
        for req in &args.print {
            print_info(args, req)?;
        }
        return Ok(());
    }

    let input = args.input.as_ref().expect("clap requires INPUT here");
    let _src = std::fs::read_to_string(input)?;

    if args.verbose {
        eprintln!(
            "compiling `{}` (opt-level={}, debuginfo={})",
            args.crate_name(),
            args.opt_level(),
            args.debuginfo_level()
        );
    }

    for emit in args.emits() {
        match emit.kind {
            Emit::Tokens => todo!("lexer"),
            Emit::Ast => todo!("parser"),
            Emit::Ir => todo!("ir"),
            Emit::DepInfo | Emit::Metadata => todo!("metadata"),
            Emit::Asm | Emit::Obj | Emit::Link => todo!("codegen"),
        }
    }

    Ok(())
}

fn print_info(args: &Args, req: &Spec<PrintKind>) -> Result {
    let out = match req.kind {
        PrintKind::CrateName => args.crate_name(),
        PrintKind::Cfg => args.cfg.join("\n"),
        PrintKind::HostTuple => {
            format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
        }
        PrintKind::SupportedCrateTypes => CrateType::value_variants()
            .iter()
            .filter_map(|v| v.to_possible_value())
            .map(|v| v.get_name().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        PrintKind::FileNames | PrintKind::Sysroot | PrintKind::TargetList => {
            todo!("{:?}", req.kind)
        }
    };

    match &req.path {
        Some(path) => std::fs::write(path, out + "\n")?,
        None => println!("{out}"),
    }
    Ok(())
}

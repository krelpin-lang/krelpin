mod args;
mod config;
mod session;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, ValueEnum};
use krelpin_ast::token::Token;
use krelpin_errors::Diagnostic;
use krelpin_sysroot::Sysroot;

pub use crate::args::{Args, CrateType, Edition, Emit, LintLevel, PrintKind, Spec};
pub use crate::config::Config;
pub use crate::session::Session;

pub fn main() -> ExitCode {
    let args = Args::parse();
    let sess = Session::new();
    run(&args, &sess);
    sess.finish()
}

pub fn run(args: &Args, sess: &Session) {
    if let Some(topic) = &args.explain {
        match config::explain(topic) {
            Some(text) => print!("{text}"),
            None => sess.dcx.emit(
                Diagnostic::error(format!("no extended information for `{topic}`"))
                    .with_help("`--explain` takes a lint name; `--explain lints` lists them"),
            ),
        }
        return;
    }

    let config = match Config::from_args(args) {
        Ok(config) => config,
        Err(errors) => {
            for error in errors {
                sess.dcx.emit(error);
            }
            return;
        }
    };
    sess.dcx.set_lint_levels(config.lints.clone());
    for name in &config.unknown_lints {
        sess.dcx.emit(
            Diagnostic::lint("unknown_lints", format!("unknown lint `{name}`"))
                .with_help("`krelpinc --explain lints` lists the lints"),
        );
    }

    if !args.print.is_empty() {
        for request in &args.print {
            print_info(args, &config, sess, request);
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
            "compiling `{}` for {} (opt-level={}, debuginfo={})",
            args.crate_name(),
            config.target.triple,
            config.opt_level,
            config.debuginfo
        );
    }

    let timer = Timer::new(config.time_passes);
    let emits = args.emits();
    let tokens = timer.time("lexing", || krelpin_parse::lexer::lex(&file, &sess.dcx));
    if sess.dcx.has_errors() {
        return;
    }

    for emit in emits.iter().filter(|emit| emit.kind == Emit::Tokens) {
        write_output(sess, emit.path.as_deref(), &render_tokens(sess, &tokens));
    }
    if emits.iter().all(|emit| emit.kind == Emit::Tokens) {
        return;
    }

    let unit = timer.time("parsing", || {
        krelpin_parse::parser::parse(tokens, &sess.dcx)
    });
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
    let mut units = timer.time("loading the standard library", || {
        sysroot.load_std(&sess.source_map, &sess.dcx)
    });
    if sess.dcx.has_errors() {
        return;
    }
    units.push(unit);
    timer.time("expansion", || {
        krelpin_expand::expand_with_cfg(&mut units, &sess.source_map, &sess.dcx, &config.cfg)
    });
    if sess.dcx.has_errors() {
        return;
    }
    let resolved = timer.time("name resolution", || {
        krelpin_resolve::resolve(&units, &sess.dcx)
    });
    if sess.dcx.has_errors() {
        return;
    }

    let typeck = timer.time("type checking", || {
        krelpin_typeck::check(&units, &resolved, &sess.dcx)
    });
    if sess.dcx.has_errors() {
        return;
    }

    let hir = timer.time("HIR lowering", || {
        krelpin_hir::lower(&units, &resolved, &typeck.tcx, &typeck.results)
    });
    let user_unit = units.len() - 1;
    for emit in emits.iter().filter(|emit| emit.kind == Emit::Hir) {
        let bodies = hir
            .bodies
            .iter()
            .filter(|body| resolved.items.def(body.owner).unit == user_unit);
        let rendered = krelpin_hir::print(bodies, &typeck.tcx);
        write_output(sess, emit.path.as_deref(), &rendered);
    }

    let mir = timer.time("MIR building", || krelpin_mir::build(&hir, &typeck.tcx));
    let all_bodies = (0..mir.bodies.len()).map(|index| krelpin_mir::BodyId(index as u32));
    for emit in emits.iter().filter(|emit| emit.kind == Emit::Mir) {
        let bodies = all_bodies
            .clone()
            .filter(|&id| resolved.items.def(mir.body(id).owner).unit == user_unit);
        let rendered = krelpin_mir::print(&mir, bodies, &typeck.tcx);
        write_output(sess, emit.path.as_deref(), &rendered);
    }
    timer.time("borrow checking", || {
        krelpin_borrowck::check(&mir, all_bodies, &typeck.tcx, &sess.dcx)
    });
    if sess.dcx.has_errors() {
        return;
    }

    for emit in emits.iter().filter(|emit| emit.kind == Emit::DepInfo) {
        let path = output_path(args, &config, emit, None);
        let sources = sysroot.std_sources().unwrap_or_default();
        let outputs: Vec<PathBuf> = emits
            .iter()
            .filter(|other| !matches!(other.kind, Emit::DepInfo | Emit::Metadata))
            .flat_map(|other| artifact_paths(args, &config, other))
            .collect();
        write_output(sess, Some(&path), &dep_info(&outputs, input, &sources));
    }

    let wants_code = |kind: Emit| matches!(kind, Emit::Ir | Emit::Asm | Emit::Obj | Emit::Link);
    if !emits.iter().any(|emit| wants_code(emit.kind)) {
        return;
    }
    let is_user = |def| resolved.items.def(def).unit == user_unit;
    let mut builds: Vec<(Option<CrateType>, krelpin_codegen_llvm::Entry)> = Vec::new();
    if args.test {
        let mut tests = Vec::new();
        for (type_def, item) in resolved.items.items(&units) {
            if resolved.items.def(type_def.def).unit != user_unit {
                continue;
            }
            for (index, member) in members(item).iter().enumerate() {
                if !member
                    .annotations
                    .iter()
                    .any(|annotation| annotation.is("Test"))
                {
                    continue;
                }
                let body = mir.bodies.iter().position(|body| {
                    body.owner == type_def.def && body.member == Some(index as u32)
                });
                let Some(body) = body.map(|index| krelpin_mir::BodyId(index as u32)) else {
                    continue;
                };
                match krelpin_codegen_llvm::check_test(&mir, &typeck.tcx, body) {
                    Ok(()) => tests.push(krelpin_codegen_llvm::TestCase {
                        body,
                        name: format!("{}.{}", item.name.name, mir.body(body).name),
                    }),
                    Err(diagnostic) => sess.dcx.emit(*diagnostic),
                }
            }
        }
        builds.push((None, krelpin_codegen_llvm::Entry::Tests(tests)));
    } else {
        for &crate_type in &config.crate_types {
            let entry = if crate_type == CrateType::Bin {
                match krelpin_codegen_llvm::find_entry(&mir, &typeck.tcx, is_user) {
                    Ok(entry) => krelpin_codegen_llvm::Entry::Main(entry),
                    Err(diagnostic) => {
                        sess.dcx.emit(*diagnostic);
                        continue;
                    }
                }
            } else {
                let prefix = args.crate_name();
                krelpin_codegen_llvm::Entry::Library {
                    exports: krelpin_codegen_llvm::library_exports(
                        &mir,
                        &typeck.tcx,
                        &prefix,
                        is_user,
                    ),
                    prefix,
                }
            };
            builds.push((Some(crate_type), entry));
        }
    }
    if sess.dcx.has_errors() {
        return;
    }
    let runtime_dir = sysroot.root().join("library/runtime");
    let code_emits: Vec<&Spec<Emit>> = emits.iter().filter(|emit| wants_code(emit.kind)).collect();
    for (crate_type, entry) in &builds {
        let ir = timer.time("code generation", || {
            krelpin_codegen_llvm::codegen(&mir, &typeck.tcx, entry, &config.target, &sess.dcx)
        });
        let Some(ir) = ir else {
            return;
        };
        for emit in &code_emits {
            let path = output_path(args, &config, emit, *crate_type);
            if emit.kind == Emit::Ir {
                write_output(sess, Some(&path), &ir);
                continue;
            }
            let kind = match (emit.kind, crate_type) {
                (Emit::Asm, _) => krelpin_codegen_llvm::OutputKind::Asm,
                (Emit::Obj, _) => krelpin_codegen_llvm::OutputKind::Object,
                (_, None | Some(CrateType::Bin)) => krelpin_codegen_llvm::OutputKind::Executable,
                (_, Some(CrateType::Dylib | CrateType::Cdylib)) => {
                    krelpin_codegen_llvm::OutputKind::SharedLib
                }
                (_, Some(_)) => krelpin_codegen_llvm::OutputKind::StaticLib,
            };
            let options = krelpin_codegen_llvm::LinkOptions {
                runtime_dir: &runtime_dir,
                target: &config.target,
                opt_level: &config.opt_level,
                debuginfo: config.debuginfo,
                target_cpu: config.target_cpu.as_deref(),
                linker: config.linker.as_deref(),
                link_args: &config.link_args,
                libraries: &config.libraries,
                keep_temps: config.keep_temps.as_deref(),
                verbose: args.verbose,
                output: &path,
                kind,
            };
            create_parent(sess, &path);
            let linked = timer.time("linking", || krelpin_codegen_llvm::link(&ir, &options));
            if let Err(message) = linked {
                sess.dcx
                    .emit(Diagnostic::error(format!("linking failed: {message}")));
            }
        }
    }
}

fn members(item: &krelpin_ast::ast::Item) -> &[krelpin_ast::ast::Member] {
    match &item.kind {
        krelpin_ast::ast::ItemKind::Class(class) => &class.members,
        krelpin_ast::ast::ItemKind::Record(def) => &def.members,
        krelpin_ast::ast::ItemKind::Interface(interface) => &interface.members,
        krelpin_ast::ast::ItemKind::Enum(def) => &def.members,
    }
}

struct Timer {
    enabled: bool,
}

impl Timer {
    fn new(enabled: bool) -> Timer {
        Timer { enabled }
    }

    fn time<T>(&self, what: &str, work: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let result = work();
        if self.enabled {
            eprintln!("time: {:>8.3}s\t{what}", start.elapsed().as_secs_f64());
        }
        result
    }
}

fn artifact_name(
    args: &Args,
    config: &Config,
    kind: Emit,
    crate_type: Option<CrateType>,
) -> String {
    let name = args.crate_name();
    let library = |extension: &str| format!("lib{name}.{extension}");
    match kind {
        Emit::Ir => format!("{name}.ll"),
        Emit::Asm => format!("{name}.s"),
        Emit::Obj => format!("{name}.o"),
        Emit::DepInfo => format!("{name}.d"),
        Emit::Link => match crate_type {
            None | Some(CrateType::Bin) => name,
            Some(CrateType::Lib | CrateType::Rlib) => library("rlib"),
            Some(CrateType::Staticlib) => library("a"),
            Some(CrateType::Dylib | CrateType::Cdylib) => library(if config.target.os == "macos" {
                "dylib"
            } else {
                "so"
            }),
        },
        Emit::Tokens | Emit::Ast | Emit::Hir | Emit::Mir | Emit::Metadata => name,
    }
}

fn output_path(
    args: &Args,
    config: &Config,
    emit: &Spec<Emit>,
    crate_type: Option<CrateType>,
) -> PathBuf {
    if let Some(path) = &emit.path {
        return path.clone();
    }
    let name = artifact_name(args, config, emit.kind, crate_type);
    let file_outputs = args
        .emits()
        .iter()
        .filter(|other| !matches!(other.kind, Emit::Metadata) && other.path.is_none())
        .count()
        * if args.test {
            1
        } else {
            config.crate_types.len().max(1)
        };
    if let Some(output) = &args.output {
        if file_outputs <= 1 {
            return output.clone();
        }
        let dir = output.parent().unwrap_or_else(|| Path::new(""));
        let stem = output.file_stem().map_or_else(
            || args.crate_name(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        let renamed = name.replacen(&args.crate_name(), &stem, 1);
        return dir.join(renamed);
    }
    match &args.out_dir {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

fn artifact_paths(args: &Args, config: &Config, emit: &Spec<Emit>) -> Vec<PathBuf> {
    if !matches!(emit.kind, Emit::Link) {
        return vec![output_path(args, config, emit, None)];
    }
    if args.test {
        return vec![output_path(args, config, emit, None)];
    }
    config
        .crate_types
        .iter()
        .map(|crate_type| output_path(args, config, emit, Some(*crate_type)))
        .collect()
}

fn dep_info(outputs: &[PathBuf], input: &Path, sources: &[PathBuf]) -> String {
    let mut files: Vec<String> = vec![input.display().to_string()];
    files.extend(sources.iter().map(|path| path.display().to_string()));
    let mut text = String::new();
    for output in outputs {
        text.push_str(&format!("{}: {}\n", output.display(), files.join(" ")));
    }
    text.push('\n');
    for file in &files {
        text.push_str(&format!("{file}:\n"));
    }
    text
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

fn print_info(args: &Args, config: &Config, sess: &Session, request: &Spec<PrintKind>) {
    let output = match request.kind {
        PrintKind::CrateName => args.crate_name(),
        PrintKind::Cfg => config.cfg.lines().join("\n"),
        PrintKind::HostTuple => krelpin_codegen_llvm::Target::host().triple,
        PrintKind::Sysroot => match locate_sysroot(args, sess) {
            Some(sysroot) => sysroot.root().display().to_string(),
            None => return,
        },
        PrintKind::SupportedCrateTypes => CrateType::value_variants()
            .iter()
            .map(|&kind| value_name(kind))
            .collect::<Vec<_>>()
            .join("\n"),
        PrintKind::TargetList => krelpin_codegen_llvm::Target::SUPPORTED.join("\n"),
        PrintKind::FileNames => {
            let names: Vec<String> = args
                .emits()
                .iter()
                .filter(|emit| !matches!(emit.kind, Emit::Metadata))
                .flat_map(|emit| artifact_paths(args, config, emit))
                .map(|path| path.display().to_string())
                .collect();
            names.join("\n")
        }
    };

    write_output(sess, request.path.as_deref(), &format!("{output}\n"));
}

fn write_output(sess: &Session, path: Option<&Path>, content: &str) {
    let Some(path) = path else {
        print!("{content}");
        return;
    };
    create_parent(sess, path);

    if let Err(error) = fs::write(path, content) {
        sess.dcx.emit(Diagnostic::error(format!(
            "couldn't write `{}`: {error}",
            path.display()
        )));
    }
}

fn create_parent(sess: &Session, path: &Path) {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return;
    };
    if let Err(error) = fs::create_dir_all(parent) {
        sess.dcx.emit(Diagnostic::error(format!(
            "couldn't create `{}`: {error}",
            parent.display()
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

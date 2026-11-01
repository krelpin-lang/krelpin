use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Spec<T> {
    pub kind: T,
    pub path: Option<PathBuf>,
}

fn parse_spec<T: ValueEnum>(s: &str) -> Result<Spec<T>, String> {
    let (kind, path) = match s.split_once('=') {
        Some((k, p)) => (k, Some(PathBuf::from(p))),
        None => (s, None),
    };
    Ok(Spec {
        kind: T::from_str(kind, false)?,
        path,
    })
}

#[derive(Parser, Debug)]
#[command(
    name = "krelpinc",
    version,
    about = "Krelpin language compiler",
    override_usage = "krelpinc [OPTIONS] INPUT",
    arg_required_else_help = true
)]
pub struct Args {
    #[arg(
        value_name = "INPUT",
        required_unless_present_any = ["print", "explain"],
        help = "Source file to compile"
    )]
    pub input: Option<PathBuf>,

    #[arg(
        long,
        value_name = "SPEC",
        help = "Configure the compilation environment. \
                SPEC supports the syntax `<NAME>[=\"<VALUE>\"]`"
    )]
    pub cfg: Vec<String>,

    #[arg(
        long,
        value_name = "SPEC",
        help = "Provide list of expected cfgs for checking"
    )]
    pub check_cfg: Vec<String>,

    #[arg(
        short = 'L',
        value_name = "[<KIND>=]<PATH>",
        help = "Add a directory to the library search path. The optional KIND \
                can be one of <dependency|crate|native|framework|all> (default: all)"
    )]
    pub lib_search_paths: Vec<String>,

    #[arg(
        short = 'l',
        value_name = "[<KIND>[:<MODIFIERS>]=]<NAME>[:<RENAME>]",
        help = "Link the generated crate(s) to the specified native library NAME. \
                The optional KIND can be one of <static|framework|dylib> (default: dylib)"
    )]
    pub link_libs: Vec<String>,

    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        default_values_t = [CrateType::Bin],
        help = "Comma separated list of types of crates for the compiler to emit"
    )]
    pub crate_type: Vec<CrateType>,

    #[arg(
        long,
        value_name = "NAME",
        help = "Specify the name of the crate being built"
    )]
    pub crate_name: Option<String>,

    #[arg(
        long,
        value_enum,
        default_value_t = Edition::E2026,
        help = "Specify which edition of the compiler to use when compiling code"
    )]
    pub edition: Edition,

    #[arg(
        long,
        value_name = "TYPE[=FILE]",
        value_delimiter = ',',
        value_parser = parse_spec::<Emit>,
        help = "Comma separated list of types of output for the compiler to emit. \
                TYPE is one of <tokens|ast|ir|asm|obj|dep-info|metadata|link> \
                (default: link)"
    )]
    pub emit: Vec<Spec<Emit>>,

    #[arg(
        long,
        value_name = "INFO[=FILE]",
        value_parser = parse_spec::<PrintKind>,
        help = "Compiler information to print on stdout (or to a file). \
                INFO is one of <cfg|crate-name|file-names|host-tuple|sysroot|\
                supported-crate-types|target-list>"
    )]
    pub print: Vec<Spec<PrintKind>>,

    #[arg(short = 'g', help = "Equivalent to -C debuginfo=2")]
    pub debuginfo: bool,

    #[arg(short = 'O', help = "Equivalent to -C opt-level=3")]
    pub optimize: bool,

    #[arg(short = 'o', value_name = "FILENAME", help = "Write output to FILENAME")]
    pub output: Option<PathBuf>,

    #[arg(
        long,
        value_name = "DIR",
        help = "Write output to compiler-chosen filename in DIR"
    )]
    pub out_dir: Option<PathBuf>,

    #[arg(
        long,
        value_name = "OPT",
        help = "Provide a detailed explanation of an error message"
    )]
    pub explain: Option<String>,

    #[arg(long, help = "Build a test harness")]
    pub test: bool,

    #[arg(
        long,
        value_name = "TARGET",
        help = "Target tuple for which the code is compiled"
    )]
    pub target: Option<String>,

    #[arg(short = 'A', long, value_name = "LINT", help = "Set lint allowed")]
    pub allow: Vec<String>,

    #[arg(short = 'W', long, value_name = "LINT", help = "Set lint warnings")]
    pub warn: Vec<String>,

    #[arg(long, value_name = "LINT", help = "Set lint force-warn")]
    pub force_warn: Vec<String>,

    #[arg(short = 'D', long, value_name = "LINT", help = "Set lint denied")]
    pub deny: Vec<String>,

    #[arg(short = 'F', long, value_name = "LINT", help = "Set lint forbidden")]
    pub forbid: Vec<String>,

    #[arg(
        long,
        value_enum,
        value_name = "LEVEL",
        help = "Set the most restrictive lint level. \
                More restrictive lints are capped at this level"
    )]
    pub cap_lints: Option<LintLevel>,

    #[arg(
        short = 'C',
        long = "codegen",
        value_name = "OPT[=VALUE]",
        help = "Set a codegen option"
    )]
    pub codegen: Vec<String>,

    #[arg(
        short = 'Z',
        value_name = "FLAG",
        hide = true,
        help = "Set an unstable option"
    )]
    pub unstable: Vec<String>,

    #[arg(short = 'v', long, help = "Use verbose output")]
    pub verbose: bool,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrateType {
    Bin,
    Lib,
    Rlib,
    Dylib,
    Cdylib,
    Staticlib,
    ProcMacro,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edition {
    #[value(name = "2026")]
    E2026,
    Future,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Emit {
    Tokens,
    Ast,
    Ir,
    Asm,
    Obj,
    DepInfo,
    Metadata,
    Link,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintKind {
    Cfg,
    CrateName,
    FileNames,
    HostTuple,
    Sysroot,
    SupportedCrateTypes,
    TargetList,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LintLevel {
    Allow,
    Warn,
    Deny,
    Forbid,
}

impl Args {
    pub fn crate_name(&self) -> String {
        if let Some(name) = &self.crate_name {
            return name.clone();
        }
        self.input
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().replace('-', "_"))
            .unwrap_or_else(|| "main".into())
    }

    pub fn emits(&self) -> Vec<Spec<Emit>> {
        if self.emit.is_empty() {
            vec![Spec {
                kind: Emit::Link,
                path: None,
            }]
        } else {
            self.emit.clone()
        }
    }

    pub fn opt_level(&self) -> u8 {
        match self.codegen_value("opt-level") {
            Some(v) => v.parse().unwrap_or(0),
            None if self.optimize => 3,
            None => 0,
        }
    }

    pub fn debuginfo_level(&self) -> u8 {
        match self.codegen_value("debuginfo") {
            Some(v) => v.parse().unwrap_or(0),
            None if self.debuginfo => 2,
            None => 0,
        }
    }

    pub fn codegen_value(&self, key: &str) -> Option<&str> {
        self.codegen
            .iter()
            .rev()
            .find_map(|c| c.strip_prefix(key)?.strip_prefix('='))
    }
}

use std::path::PathBuf;

use krelpin_codegen_llvm::Target;
use krelpin_errors::lint::{self, LINTS, WARNINGS};
use krelpin_errors::{Diagnostic, LintLevels};
use krelpin_expand::CfgSet;

use crate::args::{Args, CrateType, Edition, LintLevel};
use crate::sources::Dep;

pub struct Config {
    pub target: Target,
    pub cfg: CfgSet,
    pub lints: LintLevels,
    pub unknown_lints: Vec<String>,
    pub opt_level: String,
    pub debuginfo: u8,
    pub linker: Option<PathBuf>,
    pub link_args: Vec<String>,
    pub target_cpu: Option<String>,
    pub libraries: Vec<String>,
    pub crate_types: Vec<CrateType>,
    pub externs: Vec<(String, PathBuf)>,
    pub deps: Vec<Dep>,
    pub time_passes: bool,
    pub keep_temps: Option<PathBuf>,
}

const CODEGEN_OPTIONS: &str = "opt-level, debuginfo, linker, link-arg, link-args, target-cpu";
const UNSTABLE_OPTIONS: &str = "time-passes, keep-temps";

impl Config {
    pub fn from_args(args: &Args) -> Result<Config, Vec<Diagnostic>> {
        let mut errors = Vec::new();
        let proc_macro = args.crate_type.contains(&CrateType::ProcMacro);
        let target = match &args.target {
            Some(triple) => Target::parse(triple).unwrap_or_else(|message| {
                errors.push(Diagnostic::error(message));
                Target::host()
            }),
            None => Target::host(),
        };
        let mut config = Config {
            target,
            cfg: CfgSet::default(),
            lints: LintLevels::default(),
            unknown_lints: Vec::new(),
            opt_level: "0".to_owned(),
            debuginfo: 0,
            linker: None,
            link_args: Vec::new(),
            target_cpu: None,
            libraries: Vec::new(),
            crate_types: dedup(&args.crate_type),
            externs: Vec::new(),
            deps: Vec::new(),
            time_passes: false,
            keep_temps: None,
        };
        if args.optimize {
            config.opt_level = "3".to_owned();
        }
        if args.debuginfo {
            config.debuginfo = 2;
        }
        for option in &args.codegen {
            if let Err(message) = config.codegen_option(option) {
                errors.push(Diagnostic::error(message));
            }
        }
        for option in &args.unstable {
            if args.edition != Edition::Future {
                errors.push(
                    Diagnostic::error(format!("`-Z {option}` is an unstable option"))
                        .with_help("unstable options need `--edition future`"),
                );
                continue;
            }
            if let Err(message) = config.unstable_option(option) {
                errors.push(Diagnostic::error(message));
            }
        }
        let mut search_dirs = Vec::new();
        for path in &args.lib_search_paths {
            match search_path(path) {
                Ok(flag) => config.libraries.extend(flag),
                Err(message) => errors.push(Diagnostic::error(message)),
            }
            if let Some(dir) = crate_search_dir(path) {
                search_dirs.push(dir);
            }
        }
        for spec in &args.externs {
            match extern_crate(spec, &search_dirs) {
                Ok(found) => config.externs.push(found),
                Err(message) => errors.push(Diagnostic::error(message)),
            }
        }
        for spec in &args.deps {
            match Dep::parse(spec) {
                Ok(dep) if config.deps.iter().any(|other| other.name == dep.name) => {
                    errors.push(Diagnostic::error(format!(
                        "the library `{}` is passed to `--dep` more than once",
                        dep.name
                    )));
                }
                Ok(dep) => config.deps.push(dep),
                Err(message) => errors.push(Diagnostic::error(message)),
            }
        }
        if proc_macro && config.crate_types.len() > 1 {
            errors.push(
                Diagnostic::error("`proc-macro` cannot be combined with other crate types")
                    .with_help("build the macros and the code that uses them as separate crates"),
            );
        }
        if proc_macro && config.target != Target::host() {
            errors.push(
                Diagnostic::error("`proc-macro` crates are built for the host")
                    .with_note(format!(
                        "the compiler runs macros on `{}` while it compiles",
                        Target::host().triple
                    ))
                    .with_help("remove `--target`"),
            );
        }
        for library in &args.link_libs {
            match link_library(library, &config.target) {
                Ok(flags) => config.libraries.extend(flags),
                Err(message) => errors.push(Diagnostic::error(message)),
            }
        }
        config.configure_cfg(args, &mut errors);
        config.configure_lints(args);
        if errors.is_empty() {
            Ok(config)
        } else {
            Err(errors)
        }
    }

    pub fn builds_executable(&self, args: &Args) -> bool {
        args.test
            || self.crate_types.contains(&CrateType::Bin)
            || self.crate_types.contains(&CrateType::ProcMacro)
    }

    pub fn proc_macro(&self) -> bool {
        self.crate_types.contains(&CrateType::ProcMacro)
    }

    fn codegen_option(&mut self, option: &str) -> Result<(), String> {
        let (key, value) = match option.split_once('=') {
            Some((key, value)) => (key, Some(value)),
            None => (option, None),
        };
        let needs_value = || format!("`-C {key}` needs a value, like `-C {key}=...`");
        match key {
            "opt-level" => {
                let value = value.ok_or_else(needs_value)?;
                if !["0", "1", "2", "3", "s", "z"].contains(&value) {
                    return Err(format!(
                        "invalid `-C opt-level={value}`; expected one of 0, 1, 2, 3, s or z"
                    ));
                }
                self.opt_level = value.to_owned();
            }
            "debuginfo" => {
                self.debuginfo = match value.ok_or_else(needs_value)? {
                    "0" | "none" => 0,
                    "1" | "limited" | "line-tables-only" => 1,
                    "2" | "full" => 2,
                    other => {
                        return Err(format!(
                            "invalid `-C debuginfo={other}`; expected 0, 1, 2, none, limited \
                             or full"
                        ));
                    }
                };
            }
            "linker" => self.linker = Some(PathBuf::from(value.ok_or_else(needs_value)?)),
            "link-arg" => self
                .link_args
                .push(value.ok_or_else(needs_value)?.to_owned()),
            "link-args" => self.link_args.extend(
                value
                    .ok_or_else(needs_value)?
                    .split_whitespace()
                    .map(str::to_owned),
            ),
            "target-cpu" => self.target_cpu = Some(value.ok_or_else(needs_value)?.to_owned()),
            _ => {
                return Err(format!(
                    "unknown codegen option `{key}`; the options are {CODEGEN_OPTIONS}"
                ));
            }
        }
        Ok(())
    }

    fn unstable_option(&mut self, option: &str) -> Result<(), String> {
        let (key, value) = match option.split_once('=') {
            Some((key, value)) => (key, Some(value)),
            None => (option, None),
        };
        match (key, value) {
            ("time-passes", None) => self.time_passes = true,
            ("keep-temps", value) => {
                self.keep_temps = Some(PathBuf::from(value.unwrap_or("krelpin-temps")));
            }
            _ => {
                return Err(format!(
                    "unknown unstable option `{option}`; the options are {UNSTABLE_OPTIONS}"
                ));
            }
        }
        Ok(())
    }

    fn configure_cfg(&mut self, args: &Args, errors: &mut Vec<Diagnostic>) {
        for (name, value) in self.target.cfg() {
            self.cfg.insert(name, value.as_deref());
        }
        let edition = match args.edition {
            Edition::E2026 => "2026",
            Edition::Future => "future",
        };
        self.cfg.insert("edition", Some(edition));
        if self.opt_level == "0" {
            self.cfg.insert("debug_assertions", None);
        }
        if args.test {
            self.cfg.insert("test", None);
        }
        for spec in &args.cfg {
            if let Err(message) = self.cfg.insert_spec(spec) {
                errors.push(Diagnostic::error(message));
            }
        }
        for spec in &args.check_cfg {
            if let Err(message) = self.cfg.add_check(spec) {
                errors.push(Diagnostic::error(message));
            }
        }
    }

    fn configure_lints(&mut self, args: &Args) {
        let groups: [(&[String], krelpin_errors::LintLevel); 4] = [
            (&args.allow, krelpin_errors::LintLevel::Allow),
            (&args.warn, krelpin_errors::LintLevel::Warn),
            (&args.deny, krelpin_errors::LintLevel::Deny),
            (&args.forbid, krelpin_errors::LintLevel::Forbid),
        ];
        let check = |name: &str, unknown: &mut Vec<String>| {
            let known = lint::normalize(name) == WARNINGS || lint::lint(name).is_some();
            if !known {
                unknown.push(name.to_owned());
            }
            known
        };
        for (names, level) in groups {
            for name in names {
                if check(name, &mut self.unknown_lints) {
                    self.lints.set(name, level);
                }
            }
        }
        for name in &args.force_warn {
            if check(name, &mut self.unknown_lints) {
                self.lints.force_warn(name);
            }
        }
        if let Some(cap) = args.cap_lints {
            self.lints.cap(match cap {
                LintLevel::Allow => krelpin_errors::LintLevel::Allow,
                LintLevel::Warn => krelpin_errors::LintLevel::Warn,
                LintLevel::Deny => krelpin_errors::LintLevel::Deny,
                LintLevel::Forbid => krelpin_errors::LintLevel::Forbid,
            });
        }
    }
}

fn dedup(types: &[CrateType]) -> Vec<CrateType> {
    let mut out = Vec::new();
    for ty in types {
        if !out.contains(ty) {
            out.push(*ty);
        }
    }
    out
}

fn search_path(spec: &str) -> Result<Vec<String>, String> {
    let (kind, path) = match spec.split_once('=') {
        Some((kind, path)) => (kind, path),
        None => ("all", spec),
    };
    match kind {
        "framework" => Ok(vec![format!("-F{path}")]),
        "dependency" | "crate" | "native" | "all" => Ok(vec![format!("-L{path}")]),
        _ => Err(format!(
            "unknown library search path kind `{kind}`; expected dependency, crate, native, \
             framework or all"
        )),
    }
}

fn crate_search_dir(spec: &str) -> Option<PathBuf> {
    match spec.split_once('=') {
        Some(("dependency" | "crate" | "all", path)) => Some(PathBuf::from(path)),
        Some(_) => None,
        None => Some(PathBuf::from(spec)),
    }
}

fn extern_crate(spec: &str, search_dirs: &[PathBuf]) -> Result<(String, PathBuf), String> {
    let (name, path) = match spec.split_once('=') {
        Some((name, path)) => (name, Some(PathBuf::from(path))),
        None => (spec, None),
    };
    let valid = name
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !valid {
        return Err(format!(
            "invalid `--extern` argument `{spec}`; expected `NAME` or `NAME=PATH`"
        ));
    }
    if let Some(path) = path {
        return Ok((name.to_owned(), path));
    }
    let file = format!("lib{name}.krmacro");
    search_dirs
        .iter()
        .map(|dir| dir.join(&file))
        .find(|path| path.is_file())
        .map(|path| (name.to_owned(), path))
        .ok_or_else(|| {
            format!(
                "couldn't find the macro crate `{name}`; looked for `{file}` in the `-L` \
                 directories"
            )
        })
}

fn link_library(spec: &str, target: &Target) -> Result<Vec<String>, String> {
    let (head, name) = match spec.split_once('=') {
        Some((head, name)) => (Some(head), name),
        None => (None, spec),
    };
    if name.contains(':') {
        return Err(format!(
            "`-l {spec}` renames a native library, which only crates can do"
        ));
    }
    let (kind, modifiers) = match head {
        Some(head) => match head.split_once(':') {
            Some((kind, modifiers)) => (kind, modifiers),
            None => (head, ""),
        },
        None => ("dylib", ""),
    };
    let mut whole_archive = false;
    let mut verbatim = false;
    for modifier in modifiers.split(',').filter(|modifier| !modifier.is_empty()) {
        match modifier {
            "+whole-archive" => whole_archive = true,
            "-whole-archive" => whole_archive = false,
            "+verbatim" => verbatim = true,
            "-verbatim" => verbatim = false,
            "+bundle" | "-bundle" => {}
            _ => {
                return Err(format!(
                    "unknown library modifier `{modifier}` in `-l {spec}`"
                ));
            }
        }
    }
    let library = if verbatim {
        format!("-l:{name}")
    } else {
        format!("-l{name}")
    };
    let linux = target.os == "linux";
    match kind {
        "dylib" => Ok(vec![library]),
        "framework" => Ok(vec!["-framework".to_owned(), name.to_owned()]),
        "static" => {
            let mut flags = Vec::new();
            if linux {
                flags.push("-Wl,-Bstatic".to_owned());
            }
            if whole_archive {
                flags.push(if linux {
                    "-Wl,--whole-archive".to_owned()
                } else {
                    "-Wl,-force_load".to_owned()
                });
            }
            flags.push(library);
            if whole_archive && linux {
                flags.push("-Wl,--no-whole-archive".to_owned());
            }
            if linux {
                flags.push("-Wl,-Bdynamic".to_owned());
            }
            Ok(flags)
        }
        _ => Err(format!(
            "unknown library kind `{kind}` in `-l {spec}`; expected static, framework or dylib"
        )),
    }
}

pub fn explain(topic: &str) -> Option<String> {
    if topic == "lints" {
        let mut text = String::from("lints and their default level:\n\n");
        for lint in LINTS {
            text.push_str(&format!("    {:<24} warn    {}\n", lint.name, lint.summary));
        }
        text.push_str(&format!("\n    {WARNINGS:<24}         every lint above\n"));
        return Some(text);
    }
    lint::lint(topic).map(|lint| {
        format!(
            "{}\n\n{}\n\nIt is a warning by default; change it with `-A {}`, `-D {}` or \
             `-F {}`.\n",
            lint.summary, lint.explanation, lint.name, lint.name, lint.name
        )
    })
}

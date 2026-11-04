use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    Asm,
    Object,
    Executable,
    StaticLib,
    SharedLib,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub triple: String,
    pub arch: String,
    pub vendor: String,
    pub os: String,
    pub env: String,
}

impl Target {
    pub const SUPPORTED: &[&str] = &[
        "aarch64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-gnu",
    ];

    pub fn host() -> Target {
        let arch = env::consts::ARCH;
        let triple = match env::consts::OS {
            "macos" => format!("{arch}-apple-darwin"),
            _ => format!("{arch}-unknown-linux-gnu"),
        };
        Target::parse(&triple).unwrap_or(Target {
            triple,
            arch: arch.to_owned(),
            vendor: "unknown".to_owned(),
            os: env::consts::OS.to_owned(),
            env: String::new(),
        })
    }

    pub fn parse(triple: &str) -> Result<Target, String> {
        if !Target::SUPPORTED.contains(&triple) {
            return Err(format!(
                "unknown target `{triple}`; the supported targets are {}",
                Target::SUPPORTED
                    .iter()
                    .map(|target| format!("`{target}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let parts: Vec<&str> = triple.split('-').collect();
        let os = match parts[2] {
            "darwin" => "macos",
            other => other,
        };
        Ok(Target {
            triple: triple.to_owned(),
            arch: parts[0].to_owned(),
            vendor: parts[1].to_owned(),
            os: os.to_owned(),
            env: parts.get(3).copied().unwrap_or_default().to_owned(),
        })
    }

    pub fn cfg(&self) -> Vec<(&'static str, Option<String>)> {
        vec![
            ("target_arch", Some(self.arch.clone())),
            ("target_endian", Some("little".to_owned())),
            ("target_env", Some(self.env.clone())),
            ("target_family", Some("unix".to_owned())),
            ("target_os", Some(self.os.clone())),
            ("target_pointer_width", Some("64".to_owned())),
            ("target_vendor", Some(self.vendor.clone())),
            ("unix", None),
        ]
    }

    fn shared_flag(&self) -> &'static str {
        if self.os == "macos" {
            "-dynamiclib"
        } else {
            "-shared"
        }
    }
}

pub struct LinkOptions<'a> {
    pub runtime_dir: &'a Path,
    pub target: &'a Target,
    pub opt_level: &'a str,
    pub debuginfo: u8,
    pub target_cpu: Option<&'a str>,
    pub linker: Option<&'a Path>,
    pub link_args: &'a [String],
    pub libraries: &'a [String],
    pub keep_temps: Option<&'a Path>,
    pub verbose: bool,
    pub output: &'a Path,
    pub kind: OutputKind,
}

pub fn link(ir: &str, options: &LinkOptions<'_>) -> Result<(), String> {
    let work = scratch_dir()?;
    let result = build(ir, options, &work);
    if let Some(keep) = options.keep_temps {
        let _ = fs::create_dir_all(keep);
        if let Ok(entries) = fs::read_dir(&work) {
            for entry in entries.filter_map(Result::ok) {
                let _ = fs::copy(entry.path(), keep.join(entry.file_name()));
            }
        }
    }
    let _ = fs::remove_dir_all(&work);
    result
}

fn build(ir: &str, options: &LinkOptions<'_>, work: &Path) -> Result<(), String> {
    let ir_path = work.join("module.ll");
    fs::write(&ir_path, ir)
        .map_err(|error| format!("couldn't write `{}`: {error}", ir_path.display()))?;
    let compiler = compiler();
    let mut command = Command::new(&compiler);
    common_flags(&mut command, options);
    command.arg(format!("-O{}", options.opt_level));
    if options.debuginfo > 0 {
        command.arg(if options.debuginfo == 1 {
            "-gline-tables-only"
        } else {
            "-g"
        });
    }
    match options.kind {
        OutputKind::Asm => {
            command.args(["-S", "-x", "ir"]).arg(&ir_path);
            command.arg("-o").arg(options.output);
            run(&mut command, &compiler, options.verbose)
        }
        OutputKind::Object => {
            command.args(["-c", "-x", "ir"]).arg(&ir_path);
            command.arg("-o").arg(options.output);
            run(&mut command, &compiler, options.verbose)
        }
        OutputKind::StaticLib => {
            let object = work.join("module.o");
            command
                .args(["-c", "-x", "ir"])
                .arg(&ir_path)
                .arg("-o")
                .arg(&object);
            run(&mut command, &compiler, options.verbose)?;
            let runtime = runtime_objects(options, &compiler, work)?;
            let _ = fs::remove_file(options.output);
            let archiver =
                env::var_os("KRELPIN_AR").map_or_else(|| PathBuf::from("ar"), PathBuf::from);
            let mut archive = Command::new(&archiver);
            archive
                .arg("crs")
                .arg(options.output)
                .arg(&object)
                .args(&runtime);
            run(&mut archive, &archiver, options.verbose)
        }
        OutputKind::Executable | OutputKind::SharedLib => {
            let runtime = runtime_objects(options, &compiler, work)?;
            if let Some(linker) = options.linker {
                command.arg(format!("-fuse-ld={}", linker.display()));
            }
            if options.kind == OutputKind::SharedLib {
                command.arg(options.target.shared_flag());
            }
            command.args(["-x", "ir"]).arg(&ir_path);
            command.args(["-x", "none"]).args(&runtime);
            command.args(options.libraries);
            command.args(["-lpthread", "-lm"]);
            command.args(options.link_args);
            command.arg("-o").arg(options.output);
            run(&mut command, &compiler, options.verbose)
        }
    }
}

fn common_flags(command: &mut Command, options: &LinkOptions<'_>) {
    command.arg(format!("--target={}", options.target.triple));
    command.args(["-fPIC", "-Wno-override-module"]);
    if let Some(cpu) = options.target_cpu {
        if options.target.arch == "x86_64" {
            command.arg(format!("-march={cpu}"));
        } else {
            command.arg(format!("-mcpu={cpu}"));
        }
    }
}

fn compiler() -> PathBuf {
    env::var_os("KRELPIN_CC").map_or_else(|| PathBuf::from("clang"), PathBuf::from)
}

fn run(command: &mut Command, program: &Path, verbose: bool) -> Result<(), String> {
    if verbose {
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        eprintln!("running `{} {}`", program.display(), args.join(" "));
    }
    let output = command
        .output()
        .map_err(|error| format!("couldn't run `{}`: {error}", program.display()))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "`{}` failed:\n{}",
        program.display(),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn runtime_objects(
    options: &LinkOptions<'_>,
    compiler: &Path,
    work: &Path,
) -> Result<Vec<PathBuf>, String> {
    let runtime_dir = options.runtime_dir;
    let read_error = |error: std::io::Error| {
        format!(
            "couldn't read the runtime at `{}`: {error}",
            runtime_dir.display()
        )
    };
    let mut sources: Vec<PathBuf> = fs::read_dir(runtime_dir)
        .map_err(read_error)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "c" || ext == "h"))
        .collect();
    sources.sort();
    let mut hasher = DefaultHasher::new();
    compiler.hash(&mut hasher);
    options.target.triple.hash(&mut hasher);
    options.target_cpu.hash(&mut hasher);
    for source in &sources {
        source.file_name().hash(&mut hasher);
        fs::read(source).map_err(read_error)?.hash(&mut hasher);
    }
    let cache = cache_dir().join(format!("runtime-{:016x}", hasher.finish()));
    let c_sources: Vec<&PathBuf> = sources
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "c"))
        .collect();
    let object_name = |source: &Path| {
        format!(
            "{}.o",
            source.file_stem().unwrap_or_default().to_string_lossy()
        )
    };
    let cached: Vec<PathBuf> = c_sources
        .iter()
        .map(|source| cache.join(object_name(source)))
        .collect();
    if cached.iter().all(|object| object.exists()) {
        return Ok(cached);
    }
    let staging = work.join("runtime");
    fs::create_dir_all(&staging)
        .map_err(|error| format!("couldn't create `{}`: {error}", staging.display()))?;
    let mut built = Vec::new();
    for source in &c_sources {
        let object = staging.join(object_name(source));
        let mut command = Command::new(compiler);
        common_flags(&mut command, options);
        command
            .args(["-O2", "-std=c11", "-c"])
            .arg("-I")
            .arg(runtime_dir)
            .arg(source)
            .arg("-o")
            .arg(&object);
        run(&mut command, compiler, options.verbose)?;
        built.push(object);
    }
    if fs::create_dir_all(&cache).is_err() {
        return Ok(built);
    }
    for (object, target) in built.iter().zip(&cached) {
        let partial = target.with_extension(format!("o.{}", std::process::id()));
        if fs::copy(object, &partial).is_ok() {
            let _ = fs::rename(&partial, target);
        }
    }
    if cached.iter().all(|object| object.exists()) {
        Ok(cached)
    } else {
        Ok(built)
    }
}

fn cache_dir() -> PathBuf {
    if let Some(dir) = env::var_os("KRELPIN_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(dir).join("krelpin");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("krelpin");
    }
    env::temp_dir().join("krelpin-cache")
}

fn scratch_dir() -> Result<PathBuf, String> {
    let base = env::temp_dir();
    for attempt in 0..1000u32 {
        let dir = base.join(format!("krelpin-{}-{attempt}", std::process::id()));
        if fs::create_dir(&dir).is_ok() {
            return Ok(dir);
        }
    }
    Err("couldn't create a temporary directory".to_owned())
}

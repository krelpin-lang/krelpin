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
}

pub struct LinkOptions<'a> {
    pub runtime_dir: &'a Path,
    pub opt_level: u8,
    pub debuginfo: bool,
    pub output: &'a Path,
    pub kind: OutputKind,
}

pub fn link(ir: &str, options: &LinkOptions<'_>) -> Result<(), String> {
    let work = scratch_dir()?;
    let result = build(ir, options, &work);
    let _ = fs::remove_dir_all(&work);
    result
}

fn build(ir: &str, options: &LinkOptions<'_>, work: &Path) -> Result<(), String> {
    let ir_path = work.join("module.ll");
    fs::write(&ir_path, ir)
        .map_err(|error| format!("couldn't write `{}`: {error}", ir_path.display()))?;
    let compiler = compiler();
    let mut command = Command::new(&compiler);
    command.arg(format!("-O{}", options.opt_level.min(3)));
    command.arg("-Wno-override-module");
    if options.debuginfo {
        command.arg("-g");
    }
    match options.kind {
        OutputKind::Asm => {
            command.args(["-S", "-x", "ir"]).arg(&ir_path);
        }
        OutputKind::Object => {
            command.args(["-c", "-x", "ir"]).arg(&ir_path);
        }
        OutputKind::Executable => {
            let runtime = runtime_objects(options.runtime_dir, &compiler, work)?;
            command.args(["-x", "ir"]).arg(&ir_path);
            command.args(["-x", "none"]).args(&runtime);
            command.args(["-lpthread", "-lm"]);
        }
    }
    command.arg("-o").arg(options.output);
    run(&mut command, &compiler)
}

fn compiler() -> PathBuf {
    env::var_os("KRELPIN_CC").map_or_else(|| PathBuf::from("clang"), PathBuf::from)
}

fn run(command: &mut Command, compiler: &Path) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("couldn't run `{}`: {error}", compiler.display()))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "`{}` failed:\n{}",
        compiler.display(),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn runtime_objects(
    runtime_dir: &Path,
    compiler: &Path,
    work: &Path,
) -> Result<Vec<PathBuf>, String> {
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
    for source in &sources {
        source.file_name().hash(&mut hasher);
        fs::read(source).map_err(read_error)?.hash(&mut hasher);
    }
    let cache = cache_dir().join(format!("runtime-{:016x}", hasher.finish()));
    let objects: Vec<PathBuf> = sources
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "c"))
        .map(|source| {
            let stem = source.file_stem().unwrap_or_default().to_string_lossy();
            cache.join(format!("{stem}.o"))
        })
        .collect();
    if objects.iter().all(|object| object.exists()) {
        return Ok(objects);
    }
    let staging = work.join("runtime");
    fs::create_dir_all(&staging)
        .map_err(|error| format!("couldn't create `{}`: {error}", staging.display()))?;
    for source in sources
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "c"))
    {
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        let mut command = Command::new(compiler);
        command
            .args(["-O2", "-std=c11", "-c"])
            .arg("-I")
            .arg(runtime_dir)
            .arg(source)
            .arg("-o")
            .arg(staging.join(format!("{stem}.o")));
        run(&mut command, compiler)?;
    }
    if fs::create_dir_all(&cache).is_err() {
        return Ok(objects
            .iter()
            .map(|object| staging.join(object.file_name().unwrap_or_default()))
            .collect());
    }
    for object in &objects {
        let built = staging.join(object.file_name().unwrap_or_default());
        let partial = object.with_extension(format!("o.{}", std::process::id()));
        if fs::copy(&built, &partial).is_ok() {
            let _ = fs::rename(&partial, object);
        }
    }
    if objects.iter().all(|object| object.exists()) {
        Ok(objects)
    } else {
        Ok(objects
            .iter()
            .map(|object| staging.join(object.file_name().unwrap_or_default()))
            .collect())
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

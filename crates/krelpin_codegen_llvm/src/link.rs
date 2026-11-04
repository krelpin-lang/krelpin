use std::env;
use std::fs;
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
    let ir_path = work.join("module.ll");
    fs::write(&ir_path, ir)
        .map_err(|error| format!("couldn't write `{}`: {error}", ir_path.display()))?;
    let compiler = env::var_os("KRELPIN_CC").map_or_else(|| PathBuf::from("clang"), PathBuf::from);
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
            command.args(["-x", "ir"]).arg(&ir_path);
            command.args(["-x", "c", "-std=c11"]);
            command.arg("-I").arg(options.runtime_dir);
            let mut sources: Vec<PathBuf> = fs::read_dir(options.runtime_dir)
                .map_err(|error| {
                    format!(
                        "couldn't read the runtime at `{}`: {error}",
                        options.runtime_dir.display()
                    )
                })?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "c"))
                .collect();
            sources.sort();
            command.args(&sources);
            command.args(["-lpthread", "-lm"]);
        }
    }
    command.arg("-o").arg(options.output);
    let output = command
        .output()
        .map_err(|error| format!("couldn't run `{}`: {error}", compiler.display()))?;
    let _ = fs::remove_dir_all(&work);
    if !output.status.success() {
        return Err(format!(
            "`{}` failed:\n{}",
            compiler.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
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

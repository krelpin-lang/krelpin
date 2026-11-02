#[cfg(test)]
mod tests;

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use krelpin_ast::ast::CompilationUnit;
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_parse::{lexer, parser};
use krelpin_span::SourceMap;

const STD_DIR: &str = "library/std";
const SOURCE_EXTENSION: &str = "kr";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sysroot {
    root: PathBuf,
}

#[derive(Debug)]
pub enum SysrootError {
    NotFound,
    MissingStd(PathBuf),
}

impl SysrootError {
    pub fn into_diagnostic(self) -> Diagnostic {
        match self {
            SysrootError::NotFound => Diagnostic::error("couldn't find the standard library")
                .with_help(format!(
                    "pass `--sysroot <PATH>`, where `<PATH>/{STD_DIR}` holds the standard library"
                )),
            SysrootError::MissingStd(root) => Diagnostic::error(format!(
                "`{}` is not a sysroot: `{}` does not exist",
                root.display(),
                root.join(STD_DIR).display()
            )),
        }
    }
}

impl Sysroot {
    pub fn new(root: impl Into<PathBuf>) -> Result<Sysroot, SysrootError> {
        let root = root.into();
        if root.join(STD_DIR).is_dir() {
            Ok(Sysroot { root })
        } else {
            Err(SysrootError::MissingStd(root))
        }
    }

    pub fn locate(explicit: Option<&Path>) -> Result<Sysroot, SysrootError> {
        match explicit {
            Some(root) => Sysroot::new(root),
            None => Sysroot::from_current_exe(),
        }
    }

    fn from_current_exe() -> Result<Sysroot, SysrootError> {
        let exe = env::current_exe()
            .and_then(fs::canonicalize)
            .map_err(|_| SysrootError::NotFound)?;
        exe.ancestors()
            .skip(1)
            .find(|dir| dir.join(STD_DIR).is_dir())
            .map(|root| Sysroot {
                root: root.to_path_buf(),
            })
            .ok_or(SysrootError::NotFound)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn std_dir(&self) -> PathBuf {
        self.root.join(STD_DIR)
    }

    pub fn std_sources(&self) -> io::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        collect_sources(&self.std_dir(), &mut files)?;
        files.sort();
        Ok(files)
    }

    pub fn load_std(&self, source_map: &SourceMap, dcx: &DiagCtxt) -> Vec<CompilationUnit> {
        let files = match self.std_sources() {
            Ok(files) => files,
            Err(error) => {
                dcx.emit(Diagnostic::error(format!(
                    "couldn't read `{}`: {error}",
                    self.std_dir().display()
                )));
                return Vec::new();
            }
        };

        files
            .iter()
            .filter_map(|path| match source_map.load_file(path) {
                Ok(file) => Some(parser::parse(lexer::lex(&file, dcx), dcx)),
                Err(error) => {
                    dcx.emit(Diagnostic::error(format!(
                        "couldn't read `{}`: {error}",
                        path.display()
                    )));
                    None
                }
            })
            .collect()
    }
}

fn collect_sources(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_sources(&path, files)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == SOURCE_EXTENSION)
        {
            files.push(path);
        }
    }
    Ok(())
}

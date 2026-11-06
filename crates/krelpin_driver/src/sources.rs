//! Finding, reading and checking the source files of the crate being built
//! and of its source library dependencies.
//!
//! A crate is either a single file or a source directory. In a source
//! directory a file's package follows from where the file is: `net/http/Client.kr`
//! is in `net.http`, and the files at the root are in the default package.
//! A library's packages are under its name, the way `std` is laid out:
//! with `--dep tide=DIR`, `DIR/net/TcpStream.kr` is in `tide.net` and the
//! files at the root of `DIR` are in `tide`.

use std::path::{Path, PathBuf};

use krelpin_ast::ast::CompilationUnit;
use krelpin_ast::package::PackagePath;
use krelpin_ast::token::Token;
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_parse::{lexer, parser};

use crate::session::Session;

/// A source library passed with `--dep NAME=DIR`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dep {
    pub name: String,
    pub dir: PathBuf,
}

impl Dep {
    pub(crate) fn parse(spec: &str) -> Result<Dep, String> {
        let Some((name, dir)) = spec.split_once('=') else {
            return Err(format!("`--dep {spec}` needs the form `NAME=DIR`"));
        };
        if !is_identifier(name) {
            return Err(format!(
                "`{name}` is not a valid library name; it must be usable as a package name"
            ));
        }
        if name == "std" {
            return Err("`std` is the standard library and cannot be passed to `--dep`".into());
        }
        let dir = PathBuf::from(dir);
        if !dir.is_dir() {
            return Err(format!(
                "the library `{name}` must be a source directory, and `{}` is not one",
                dir.display()
            ));
        }
        Ok(Dep {
            name: name.to_string(),
            dir,
        })
    }
}

/// Where a crate's source comes from, and which package its root holds.
pub(crate) struct Root<'a> {
    pub path: &'a Path,
    /// The package of the files at the root of a source directory: the
    /// library name for a library, nothing for a program.
    pub package: Option<&'a str>,
}

/// The files of a crate, lexed.
pub(crate) struct Lexed {
    pub files: Vec<PathBuf>,
    pub tokens: Vec<Vec<Token>>,
    /// The directory the packages are relative to, or `None` for a single file.
    pub dir: Option<PathBuf>,
}

pub(crate) fn lex(root: &Root<'_>, sess: &Session) -> Option<Lexed> {
    let (files, dir) = if root.path.is_dir() {
        match krelpin_sysroot::source_files(root.path) {
            Ok(files) if files.is_empty() => {
                sess.dcx.emit(Diagnostic::error(format!(
                    "`{}` holds no `.kr` files",
                    root.path.display()
                )));
                return None;
            }
            Ok(files) => (files, Some(root.path.to_path_buf())),
            Err(error) => {
                sess.dcx.emit(Diagnostic::error(format!(
                    "couldn't read `{}`: {error}",
                    root.path.display()
                )));
                return None;
            }
        }
    } else {
        (vec![root.path.to_path_buf()], None)
    };
    let mut tokens = Vec::with_capacity(files.len());
    for path in &files {
        match sess.source_map.load_file(path) {
            Ok(file) => tokens.push(lexer::lex(&file, &sess.dcx)),
            Err(error) => {
                sess.dcx.emit(Diagnostic::error(format!(
                    "couldn't read `{}`: {error}",
                    path.display()
                )));
                return None;
            }
        }
    }
    Some(Lexed { files, tokens, dir })
}

pub(crate) fn parse(lexed: Lexed, root: &Root<'_>, dcx: &DiagCtxt) -> Vec<CompilationUnit> {
    let units: Vec<CompilationUnit> = lexed
        .tokens
        .into_iter()
        .map(|tokens| parser::parse(tokens, dcx))
        .collect();
    if let Some(dir) = &lexed.dir {
        for (path, unit) in lexed.files.iter().zip(&units) {
            check_package(dir, path, unit, root.package, dcx);
        }
    }
    units
}

/// Reads, lexes and parses a source library.
pub(crate) fn load_dep(dep: &Dep, sess: &Session) -> Vec<CompilationUnit> {
    let root = Root {
        path: &dep.dir,
        package: Some(&dep.name),
    };
    match lex(&root, sess) {
        Some(lexed) => parse(lexed, &root, &sess.dcx),
        None => Vec::new(),
    }
}

/// Checks that a file in a source directory declares the package its
/// directory stands for.
fn check_package(
    dir: &Path,
    path: &Path,
    unit: &CompilationUnit,
    root_package: Option<&str>,
    dcx: &DiagCtxt,
) {
    let relative = path.strip_prefix(dir).unwrap_or(path);
    let mut segments: Vec<String> = root_package.into_iter().map(str::to_string).collect();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            let name = component.as_os_str().to_string_lossy().into_owned();
            if !is_identifier(&name) {
                dcx.emit(
                    Diagnostic::error(format!(
                        "`{}` is in the directory `{name}`, which is not a valid package name",
                        relative.display()
                    ))
                    .with_help("rename the directory to a name made of letters, digits and `_`"),
                );
                return;
            }
            segments.push(name);
        }
    }
    let expected = segments.join(".");
    let declared = PackagePath::of_unit(unit).to_string();
    if declared == expected {
        return;
    }
    let span = unit
        .package
        .as_ref()
        .map_or(unit.span.shrink_to_lo(), |package| package.span);
    let diagnostic = if expected.is_empty() {
        Diagnostic::error(format!(
            "`{}` is at the root of the source directory, so it must not declare a package",
            relative.display()
        ))
        .with_span(span)
        .with_help(format!(
            "remove the `package` line, or move the file into `{}`",
            declared.replace('.', "/")
        ))
    } else {
        Diagnostic::error(format!(
            "`{}` must be in the package `{expected}`",
            relative.display()
        ))
        .with_span(span)
        .with_note(format!(
            "the package of a file follows from its directory in `{}`",
            dir.display()
        ))
        .with_help(if declared.is_empty() {
            format!("add `package {expected};` at the top of the file")
        } else {
            format!(
                "declare `package {expected};`, or move the file to the directory of `{declared}`"
            )
        })
    };
    dcx.emit(diagnostic);
}

/// Checks that each crate keeps to its own packages: only the standard
/// library is in `std`, and only a library is in the packages named after it.
pub(crate) fn check_namespaces(units: &[CompilationUnit], deps: &[Dep], dcx: &DiagCtxt) {
    for unit in units {
        let package = PackagePath::of_unit(unit);
        let span = unit
            .package
            .as_ref()
            .map_or(unit.span.shrink_to_lo(), |path| path.span);
        if package.is_std() {
            dcx.emit(
                Diagnostic::error(format!(
                    "the package `{package}` is reserved for the standard library"
                ))
                .with_span(span)
                .with_help("choose a package name that doesn't start with `std`"),
            );
            continue;
        }
        let first = package.to_string();
        let first = first.split('.').next().unwrap_or_default();
        if let Some(dep) = deps.iter().find(|dep| dep.name == first) {
            dcx.emit(
                Diagnostic::error(format!(
                    "the package `{package}` belongs to the library `{}`",
                    dep.name
                ))
                .with_span(span)
                .with_note(format!(
                    "`--dep {}=...` claims `{}` and its subpackages",
                    dep.name, dep.name
                ))
                .with_help("choose a package name that doesn't start with the library's name"),
            );
        }
    }
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

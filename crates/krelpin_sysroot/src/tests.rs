use std::path::{Path, PathBuf};

use krelpin_ast::ast::CompilationUnit;
use krelpin_errors::{DiagCtxt, DiagnosticBuffer};
use krelpin_span::SourceMap;

use crate::{Sysroot, SysrootError};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root exists")
}

fn sysroot() -> Sysroot {
    Sysroot::new(workspace_root()).expect("the workspace is a sysroot")
}

fn load() -> (Vec<PathBuf>, Vec<CompilationUnit>, Vec<String>) {
    let sysroot = sysroot();
    let buffer = DiagnosticBuffer::default();
    let dcx = DiagCtxt::new(buffer.clone());
    let units = sysroot.load_std(&SourceMap::new(), &dcx);
    let files = sysroot.std_sources().expect("library/std is readable");
    (files, units, buffer.messages())
}

fn relative_dir(file: &Path) -> String {
    let relative = file.strip_prefix(sysroot().std_dir()).unwrap();
    let dir = relative.parent().unwrap();
    dir.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(".")
}

#[test]
fn located_from_the_compiler_executable() {
    assert_eq!(Sysroot::locate(None).unwrap(), sysroot());
}

#[test]
fn explicit_sysroot_must_contain_std() {
    let root = workspace_root().join("crates");
    assert!(matches!(
        Sysroot::locate(Some(&root)),
        Err(SysrootError::MissingStd(path)) if path == root
    ));
    assert_eq!(Sysroot::locate(Some(&workspace_root())).unwrap(), sysroot());
}

#[test]
fn std_parses_without_errors() {
    let (files, units, errors) = load();
    assert_eq!(errors, Vec::<String>::new());
    assert!(!files.is_empty());
    assert_eq!(units.len(), files.len());
}

#[test]
fn packages_match_directories() {
    let (files, units, _) = load();
    for (file, unit) in files.iter().zip(&units) {
        let package: Vec<&str> = unit
            .package
            .iter()
            .flat_map(|path| &path.segments)
            .map(|segment| segment.name.as_str())
            .collect();
        let expected = format!("std.{}", relative_dir(file));
        assert_eq!(package.join("."), expected, "{}", file.display());
    }
}

#[test]
fn each_file_declares_the_type_or_macro_it_is_named_after() {
    let (files, units, _) = load();
    for (file, unit) in files.iter().zip(&units) {
        let names: Vec<&str> = unit
            .items
            .iter()
            .map(|item| item.name)
            .chain(unit.macros.iter().map(|def| def.name))
            .map(|name| name.name.as_str())
            .collect();
        let stem = file.file_stem().unwrap().to_string_lossy();
        assert_eq!(names, [stem.as_ref()], "{}", file.display());
    }
}

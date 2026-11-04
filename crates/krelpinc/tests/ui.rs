use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BLESS_VAR: &str = "KRELPIN_BLESS";

#[test]
fn ui() {
    let root = workspace_root();
    let bless = env::var_os(BLESS_VAR).is_some();

    let mut files = Vec::new();
    collect_sources(&root.join("tests/ui"), &mut files);
    files.sort();
    assert!(!files.is_empty(), "no ui tests found");

    let failures: Vec<String> = files
        .iter()
        .filter_map(|file| run_test(&root, file, bless))
        .collect();

    assert!(
        failures.is_empty(),
        "{} ui test(s) failed, rerun with `{BLESS_VAR}=1` to update the expected output\n\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root exists")
}

fn collect_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("readable directory entry").path();
        if path.is_dir() {
            collect_sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "kr") {
            files.push(path);
        }
    }
}

fn run_test(root: &Path, file: &Path, bless: bool) -> Option<String> {
    let relative = file
        .strip_prefix(root)
        .expect("test file is inside the workspace");
    let emit = if relative.starts_with("tests/ui/lexer") {
        "--emit=tokens"
    } else if relative.starts_with("tests/ui/parser") {
        "--emit=ast"
    } else if relative.starts_with("tests/ui/hir") {
        "--emit=hir"
    } else if relative.starts_with("tests/ui/mir") {
        "--emit=mir"
    } else if relative.starts_with("tests/ui/codegen") {
        "--emit=ir=/dev/null"
    } else {
        "--emit=metadata"
    };
    let output = Command::new(env!("CARGO_BIN_EXE_krelpinc"))
        .current_dir(root)
        .arg(relative)
        .arg(emit)
        .output()
        .expect("krelpinc runs");

    let outputs = [("stdout", &output.stdout), ("stderr", &output.stderr)];
    let mismatches: Vec<String> = outputs
        .into_iter()
        .filter_map(|(stream, actual)| {
            let actual = String::from_utf8_lossy(actual);
            compare(
                relative,
                &file.with_extension(stream),
                stream,
                &actual,
                bless,
            )
        })
        .collect();

    (!mismatches.is_empty()).then(|| mismatches.join("\n"))
}

fn compare(
    test: &Path,
    expected_path: &Path,
    stream: &str,
    actual: &str,
    bless: bool,
) -> Option<String> {
    if bless {
        if actual.is_empty() {
            let _ = fs::remove_file(expected_path);
        } else {
            fs::write(expected_path, actual).expect("expected output is writable");
        }
        return None;
    }

    let expected = fs::read_to_string(expected_path).unwrap_or_default();
    (expected != actual).then(|| {
        format!(
            "{}: {stream} differs\n--- expected\n{expected}\n+++ actual\n{actual}",
            test.display()
        )
    })
}

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root exists")
}

fn krelpinc(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_krelpinc"))
        .current_dir(root())
        .args(args)
        .output()
        .expect("krelpinc runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn scratch(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("krelpin-cli-{}-{name}", std::process::id()));
    fs::create_dir_all(&dir).expect("temporary directory is writable");
    dir
}

fn has_clang() -> bool {
    Command::new("clang").arg("--version").output().is_ok()
}

const SOURCE: &str = "tests/cli/features.kr";

#[test]
fn prints_compiler_information() {
    let cfg = stdout(&krelpinc(&[
        SOURCE,
        "--print",
        "cfg",
        "--cfg",
        "feature=\"loud\"",
    ]));
    assert!(cfg.contains("target_family=\"unix\""), "{cfg}");
    assert!(cfg.contains("feature=\"loud\""), "{cfg}");
    assert!(cfg.contains("edition=\"2026\""), "{cfg}");
    let files = stdout(&krelpinc(&[
        SOURCE,
        "--crate-type",
        "bin,staticlib",
        "--emit=link,ir",
        "--print",
        "file-names",
    ]));
    assert_eq!(files, "features\nlibfeatures.a\nfeatures.ll\n");
    let targets = stdout(&krelpinc(&["--print", "target-list"]));
    assert!(targets.contains("x86_64-unknown-linux-gnu"), "{targets}");
    let types = stdout(&krelpinc(&["--print", "supported-crate-types"]));
    assert_eq!(types, "bin\nlib\nrlib\ndylib\ncdylib\nstaticlib\n");
}

#[test]
fn explains_lints() {
    let lints = krelpinc(&["--explain", "lints"]);
    assert!(stdout(&lints).contains("unreachable_patterns"));
    let lint = krelpinc(&["--explain", "unreachable-patterns"]);
    assert!(stdout(&lint).starts_with("a `case` that can never match"));
    let unknown = krelpinc(&["--explain", "E0001"]);
    assert!(!unknown.status.success());
}

#[test]
fn lint_flags_change_the_level() {
    let default = krelpinc(&[SOURCE, "--emit=metadata"]);
    assert!(default.status.success());
    assert!(stderr(&default).contains("warning: unreachable pattern"));
    let allowed = krelpinc(&[SOURCE, "--emit=metadata", "-A", "unreachable_patterns"]);
    assert_eq!(stderr(&allowed), "");
    let denied = krelpinc(&[SOURCE, "--emit=metadata", "-D", "warnings"]);
    assert!(!denied.status.success());
    assert!(stderr(&denied).contains("`-D warnings` is set"));
    let capped = krelpinc(&[
        SOURCE,
        "--emit=metadata",
        "-D",
        "warnings",
        "--cap-lints",
        "warn",
    ]);
    assert!(capped.status.success());
    let unknown = krelpinc(&[SOURCE, "--emit=metadata", "-A", "warnings", "-W", "typo"]);
    assert_eq!(stderr(&unknown), "");
}

#[test]
fn rejects_invalid_options() {
    let opt = krelpinc(&[SOURCE, "-C", "opt-level=9"]);
    assert!(stderr(&opt).contains("invalid `-C opt-level=9`"));
    let codegen = krelpinc(&[SOURCE, "-C", "bogus"]);
    assert!(stderr(&codegen).contains("unknown codegen option `bogus`"));
    let unstable = krelpinc(&[SOURCE, "-Z", "time-passes"]);
    assert!(stderr(&unstable).contains("unstable options need `--edition future`"));
    let unknown = krelpinc(&[SOURCE, "--edition", "future", "-Z", "nothing"]);
    assert!(stderr(&unknown).contains("unknown unstable option `nothing`"));
    let target = krelpinc(&[SOURCE, "--target", "sparc-sun-solaris"]);
    assert!(stderr(&target).contains("unknown target `sparc-sun-solaris`"));
    let library = krelpinc(&[SOURCE, "-l", "odd=m"]);
    assert!(stderr(&library).contains("unknown library kind `odd`"));
}

#[test]
fn writes_dependency_information() {
    let dir = scratch("deps");
    let deps = dir.join("features.d");
    let output = krelpinc(&[
        SOURCE,
        "-A",
        "warnings",
        &format!("--emit=dep-info={}", deps.display()),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let text = fs::read_to_string(&deps).expect("dep-info is written");
    assert!(text.contains(SOURCE), "{text}");
    assert!(text.contains("library/std/lang/String.kr"), "{text}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn builds_programs_tests_and_libraries() {
    if !has_clang() {
        return;
    }
    let dir = scratch("build");
    let dir_arg = dir.display().to_string();
    let base = ["-A", "warnings", "--out-dir", &dir_arg];
    let program = krelpinc(&[&[SOURCE, "--cfg", "feature=\"loud\""][..], &base].concat());
    assert!(program.status.success(), "{}", stderr(&program));
    let run = Command::new(dir.join("features"))
        .output()
        .expect("program runs");
    assert_eq!(stdout(&run), "linux LOUD 49\nthree\n");

    let tests = krelpinc(
        &[
            &[SOURCE, "--test", "-o"][..],
            &[&format!("{dir_arg}/tests")],
            &base[..2],
        ]
        .concat(),
    );
    assert!(tests.status.success(), "{}", stderr(&tests));
    let run = Command::new(dir.join("tests")).output().expect("tests run");
    assert_eq!(run.status.code(), Some(101));
    let report = stdout(&run);
    assert!(
        report.contains("test Main.squaresNumbers ... ok"),
        "{report}"
    );
    assert!(
        report.contains("test Main.failsOnPurpose ... FAILED"),
        "{report}"
    );
    assert!(
        report.contains("test result: FAILED. 1 passed; 1 failed"),
        "{report}"
    );

    let library = krelpinc(&[&[SOURCE, "--crate-type", "staticlib,cdylib"][..], &base].concat());
    assert!(library.status.success(), "{}", stderr(&library));
    assert!(dir.join("libfeatures.a").exists());
    assert!(dir.join("libfeatures.so").exists());
    let symbols = Command::new("nm")
        .arg(dir.join("libfeatures.a"))
        .output()
        .map(|output| stdout(&output))
        .unwrap_or_default();
    if !symbols.is_empty() {
        assert!(symbols.contains("features_MathLib_square"), "{symbols}");
        assert!(symbols.contains("features_init"), "{symbols}");
    }
    let _ = fs::remove_dir_all(dir);
}

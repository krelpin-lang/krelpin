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
    assert_eq!(
        types,
        "bin\nlib\nrlib\ndylib\ncdylib\nstaticlib\nproc-macro\n"
    );
    let macro_files = stdout(&krelpinc(&[
        "tests/cli/macros/accessors.kr",
        "--crate-type",
        "proc-macro",
        "--print",
        "file-names",
    ]));
    assert_eq!(macro_files, "libaccessors.krmacro\n");
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

fn build_macro_crate(name: &str, dir: &Path) {
    let source = format!("tests/cli/macros/{name}.kr");
    let built = krelpinc(&[
        &source,
        "--crate-type",
        "proc-macro",
        "--out-dir",
        dir.to_str().expect("utf-8 path"),
    ]);
    assert!(built.status.success(), "{}", stderr(&built));
    assert!(dir.join(format!("lib{name}.krmacro")).is_file());
}

#[test]
fn runs_macros_from_proc_macro_crates() {
    if !has_clang() {
        return;
    }
    let dir = scratch("macros");
    build_macro_crate("accessors", &dir);
    let dir_arg = dir.to_str().expect("utf-8 path");
    let program = dir.join("app");
    let built = krelpinc(&[
        "tests/cli/macros/app.kr",
        "--extern",
        "accessors",
        "-L",
        dir_arg,
        "-o",
        program.to_str().expect("utf-8 path"),
    ]);
    assert!(built.status.success(), "{}", stderr(&built));
    let run = Command::new(&program).output().expect("the program runs");
    assert_eq!(stdout(&run), "-> main\n42\n-> sum\n42\nhello from Point\n");

    let library = dir.join("libaccessors.krmacro");
    let explicit = format!("accessors={}", library.display());
    let checked = krelpinc(&[
        "tests/cli/macros/app.kr",
        "--extern",
        &explicit,
        "--emit=metadata",
    ]);
    assert!(checked.status.success(), "{}", stderr(&checked));

    let renamed = format!("other={}", library.display());
    let wrong_name = krelpinc(&["tests/cli/macros/app.kr", "--extern", &renamed]);
    assert!(stderr(&wrong_name).contains("is the macro crate `accessors`, not `other`"));
    let not_macros = format!("accessors={}", program.display());
    let wrong_file = krelpinc(&["tests/cli/macros/app.kr", "--extern", &not_macros]);
    assert!(stderr(&wrong_file).contains("is not a macro crate"));
    let missing = krelpinc(&["tests/cli/macros/app.kr", "--extern", "accessors"]);
    assert!(stderr(&missing).contains("couldn't find the macro crate `accessors`"));
    let without = krelpinc(&["tests/cli/macros/app.kr", "--emit=metadata"]);
    assert!(stderr(&without).contains("cannot find macro `Getters` in this scope"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reports_macro_failures() {
    if !has_clang() {
        return;
    }
    let dir = scratch("macro-failures");
    build_macro_crate("broken", &dir);
    let output = krelpinc(&[
        "tests/cli/macros/misuse.kr",
        "--extern",
        "broken",
        "-L",
        dir.to_str().expect("utf-8 path"),
        "--emit=metadata",
    ]);
    assert!(!output.status.success());
    let errors = stderr(&output);
    for expected in [
        "error: fields are not allowed here\n --> tests/cli/macros/misuse.kr:5:9",
        "about to crash",
        "error: the macro `@Crash` panicked",
        "it threw IndexOutOfBoundsException: Index 3 out of bounds for length 1",
        "error: the macro `@Junk` produced an invalid token",
        "`a b` is not a single token",
        "error: recursion limit reached while expanding `@Again`",
        "this comes from 64 nested expansions of `@Again`",
        "error: `@Keep` cannot be used on enum variants",
        "aborting due to 5 previous errors",
    ] {
        assert!(
            errors.contains(expected),
            "missing `{expected}` in:\n{errors}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn checks_proc_macro_options() {
    let combined = krelpinc(&[
        "tests/cli/macros/accessors.kr",
        "--crate-type",
        "proc-macro,bin",
    ]);
    assert!(stderr(&combined).contains("`proc-macro` cannot be combined with other crate types"));
    let foreign = if cfg!(target_arch = "x86_64") {
        "aarch64-unknown-linux-gnu"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let cross = krelpinc(&[
        "tests/cli/macros/accessors.kr",
        "--crate-type",
        "proc-macro",
        "--target",
        foreign,
    ]);
    assert!(stderr(&cross).contains("`proc-macro` crates are built for the host"));
    let invalid = krelpinc(&[SOURCE, "--extern", "not-a-name=x"]);
    assert!(stderr(&invalid).contains("invalid `--extern` argument `not-a-name=x`"));
    let outside = krelpinc(&["tests/cli/macros/accessors.kr", "--emit=metadata"]);
    assert!(stderr(&outside).contains("macros can only be defined in a `proc-macro` crate"));
}

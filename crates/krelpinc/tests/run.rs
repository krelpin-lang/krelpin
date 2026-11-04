use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BLESS_VAR: &str = "KRELPIN_BLESS";
const VALGRIND_VAR: &str = "KRELPIN_VALGRIND";
const OPT_LEVELS: [&str; 2] = ["0", "3"];

struct Options<'a> {
    root: &'a Path,
    out_dir: &'a Path,
    bless: bool,
    valgrind: bool,
}

#[test]
fn run() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root exists");
    if Command::new("clang").arg("--version").output().is_err() {
        eprintln!("skipping run tests: `clang` is not installed");
        return;
    }
    let mut files: Vec<PathBuf> = fs::read_dir(root.join("tests/run"))
        .expect("tests/run exists")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "kr"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no run tests found");
    let out_dir = env::temp_dir().join(format!("krelpin-run-{}", std::process::id()));
    fs::create_dir_all(&out_dir).expect("temporary directory is writable");
    let options = Options {
        root: &root,
        out_dir: &out_dir,
        bless: env::var_os(BLESS_VAR).is_some(),
        valgrind: env::var_os(VALGRIND_VAR).is_some(),
    };
    let levels: &[&str] = if options.bless {
        &OPT_LEVELS[..1]
    } else {
        &OPT_LEVELS
    };
    let failures: Vec<String> = files
        .iter()
        .flat_map(|file| levels.iter().map(move |opt| (file, *opt)))
        .filter_map(|(file, opt)| run_test(&options, file, opt))
        .collect();
    let _ = fs::remove_dir_all(&out_dir);
    assert!(
        failures.is_empty(),
        "{} run test(s) failed, rerun with `{BLESS_VAR}=1` to update the expected output\n\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

fn run_test(options: &Options<'_>, file: &Path, opt: &str) -> Option<String> {
    let relative = file
        .strip_prefix(options.root)
        .expect("test file is inside the workspace");
    let name = format!("{} (-O{opt})", relative.display());
    let stem = file
        .file_stem()
        .expect("test file has a name")
        .to_string_lossy();
    let exe = options.out_dir.join(format!("{stem}-O{opt}"));
    let build = Command::new(env!("CARGO_BIN_EXE_krelpinc"))
        .current_dir(options.root)
        .arg(relative)
        .arg("-C")
        .arg(format!("opt-level={opt}"))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("krelpinc runs");
    if !build.status.success() {
        return Some(format!(
            "{name}: failed to compile\n{}",
            String::from_utf8_lossy(&build.stderr)
        ));
    }
    let mut command = if options.valgrind {
        let mut command = Command::new("valgrind");
        command
            .args(["-q", "--leak-check=full", "--error-exitcode=99"])
            .arg(&exe);
        command
    } else {
        Command::new(&exe)
    };
    if let Ok(input) = fs::File::open(file.with_extension("stdin")) {
        command.stdin(input);
    }
    let output = command
        .current_dir(options.root)
        .output()
        .expect("the test program runs");
    let code = output.status.code().unwrap_or(-1);
    let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if code != 0 {
        stderr.push_str(&format!("exit code: {code}\n"));
    }
    let outputs = [
        (
            "stdout",
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ),
        ("stderr", stderr),
    ];
    let mismatches: Vec<String> = outputs
        .iter()
        .filter_map(|(stream, actual)| compare(options, file, &name, stream, actual))
        .collect();
    (!mismatches.is_empty()).then(|| mismatches.join("\n"))
}

fn compare(
    options: &Options<'_>,
    file: &Path,
    name: &str,
    stream: &str,
    actual: &str,
) -> Option<String> {
    let expected_path = file.with_extension(stream);
    if options.bless {
        if actual.is_empty() {
            let _ = fs::remove_file(&expected_path);
        } else {
            fs::write(&expected_path, actual).expect("expected output is writable");
        }
        return None;
    }
    let expected = fs::read_to_string(&expected_path).unwrap_or_default();
    (expected != actual).then(|| {
        format!("{name}: {stream} differs\n--- expected\n{expected}\n+++ actual\n{actual}")
    })
}

//! Bad command-line values must produce `error: ...` on stderr and exit code 2 (never a
//! panic, which would exit 101). Drives the real `cp_bench` binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bench(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cp_bench"))
        .current_dir(root())
        .args(["--ticks", "20", "--warmup", "2"])
        .args(args)
        .output()
        .unwrap()
}

fn assert_rejected(args: &[&str], needle: &str) {
    let o = bench(args);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(2), "args {args:?}: {err}");
    assert!(err.starts_with("error: "), "args {args:?}: {err}");
    assert!(err.contains(needle), "args {args:?}: {err}");
    assert!(!err.contains("panicked"), "args {args:?}: {err}");
}

#[test]
fn bad_inject_sleep_values_exit_2_with_a_message() {
    for v in [
        "Agents=-1",
        "Agents=-0.5",
        "Agents=NaN",
        "Agents=inf",
        "Agents=-inf",
    ] {
        assert_rejected(&["--inject-sleep", v], "--inject-sleep");
    }
    assert_rejected(&["--inject-sleep", "Agents=abc"], "not a number");
    assert_rejected(&["--inject-sleep", "Agents="], "not a number");
    assert_rejected(&["--inject-sleep", "Agents=1e300"], "out of range");
    assert_rejected(&["--inject-sleep", "Nope=1"], "unknown phase");
    assert_rejected(&["--inject-sleep", "Agents"], "Phase=MS");
}

#[test]
fn valid_inject_sleep_still_works() {
    for v in ["Agents=0", "Agents=0.01", "Agents=0.0"] {
        let o = bench(&["--inject-sleep", v]);
        assert!(
            o.status.success(),
            "{v}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn bad_content_dir_prints_display_not_debug() {
    let o = bench(&[
        "--save",
        "fixtures/saves/v1-basic.sav",
        "--content",
        "/nonexistent",
    ]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(2), "{err}");
    assert!(err.starts_with("error: --content /nonexistent: "), "{err}");
    assert!(
        !err.contains("ContentErrors(") && !err.contains("ContentError {"),
        "{err}"
    );
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn content_errors_are_one_line_each() {
    // A content dir with two broken defs: every problem is its own `error:` line.
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bad-content");
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["roads", "buildings", "services", "vehicles"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    std::fs::write(dir.join("roads/a.ron"), "not ron at all").unwrap();
    std::fs::write(dir.join("vehicles/b.ron"), "also not ron").unwrap();
    let o = bench(&[
        "--save",
        "fixtures/saves/v1-basic.sav",
        "--content",
        dir.to_str().unwrap(),
    ]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(2), "{err}");
    let lines: Vec<&str> = err.lines().collect();
    assert!(lines.len() >= 2, "{err}");
    assert!(
        lines.iter().all(|l| l.starts_with("error: --content ")),
        "{err}"
    );
}

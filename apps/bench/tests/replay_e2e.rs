//! Replay round trip through the real `cp_bench` binary: `--save` + N ticks, twice,
//! must give an identical checksum, equal to a continuous in-process run.

use cp_core::{SimSpeed, Tick};
use cp_sim::{SimCommand, SimRunner, run_headless};
use std::path::{Path, PathBuf};
use std::process::Command;

const SEED: u64 = 5;

fn cmds() -> Vec<(Tick, SimCommand)> {
    vec![
        (
            Tick(10),
            SimCommand::SetSpeed {
                speed: SimSpeed::X3,
            },
        ),
        (
            Tick(900),
            SimCommand::SetSpeed {
                speed: SimSpeed::X2,
            },
        ),
    ]
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bench(args: &[&str], tag: &str) -> serde_json::Value {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{tag}.json"));
    let _ = std::fs::remove_file(&out);
    let o = Command::new(env!("CARGO_BIN_EXE_cp_bench"))
        .current_dir(root())
        .args(args)
        .args(["--out", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap()
}

#[test]
fn save_then_n_ticks_twice_is_identical_and_equals_a_continuous_run() {
    // Save at tick 700 of a sim that has queued, applied and still-pending commands.
    let mut sim = SimRunner::new(SEED);
    for (t, c) in cmds() {
        sim.push_command(c, t);
    }
    for _ in 0..700 {
        sim.tick_once();
    }
    let content = cp_content::ContentDb::load_dir(root().join("assets/content")).unwrap();
    let bytes = cp_save::save_runner(&sim, content.content_hash(), "flatlands").unwrap();
    let save = Path::new(env!("CARGO_TARGET_TMPDIR")).join("replay.sav");
    std::fs::write(&save, bytes).unwrap();
    let save = save.to_str().unwrap();

    let a = bench(&["--save", save, "--ticks", "500", "--warmup", "0"], "a");
    let b = bench(&["--save", save, "--ticks", "500", "--warmup", "0"], "b");
    assert_eq!(
        a["checksum"], b["checksum"],
        "two runs of the same save differ"
    );
    assert_eq!(a["save"]["tick"], 700);
    assert!(a["save"]["warnings"].as_array().unwrap().is_empty());

    // Warmup ticks are real ticks: 50 + 450 reaches the same state as 0 + 500.
    let w = bench(&["--save", save, "--ticks", "450", "--warmup", "50"], "w");
    assert_eq!(a["checksum"], w["checksum"]);
    // The seed flag cannot influence a loaded save (RNG state comes from the file).
    let s = bench(
        &[
            "--save", save, "--ticks", "500", "--warmup", "0", "--seed", "99",
        ],
        "s",
    );
    assert_eq!(a["checksum"], s["checksum"]);
    // Wall-clock perturbation must not change sim state.
    let z = bench(
        &[
            "--save",
            save,
            "--ticks",
            "500",
            "--warmup",
            "0",
            "--inject-sleep",
            "Economy=0.2",
        ],
        "z",
    );
    assert_eq!(a["checksum"], z["checksum"]);

    // Equal to a run that never went through a save: 700 + 500 ticks from scratch.
    let want = format!("{:016x}", run_headless(SEED, &cmds(), 1200));
    assert_eq!(a["checksum"], want.as_str());
}

#[test]
fn bad_flags_exit_with_code_2_and_a_message() {
    for args in [
        &["--ticks", "0"][..],
        &["--bogus"],
        &["--save", "/nope.sav"],
        &["--save", "a", "--scenario", "empty"],
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_cp_bench"))
            .current_dir(root())
            .args(args)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&o.stderr).starts_with("error: "),
            "{args:?}"
        );
    }
}

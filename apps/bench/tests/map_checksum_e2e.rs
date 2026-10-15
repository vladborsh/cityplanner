//! `cp_bench --map M --ticks N --warmup 0 --seed 1` must start from the same sim state as the
//! client (`cp_desktop --map M --hour 0 --headless N`), so their checksums are comparable.
//! The client's setup is `cp_app::build_sim`; this runs it in-process (no window) and compares
//! with the real `cp_bench` binary.

use std::path::{Path, PathBuf};
use std::process::Command;

use cp_core::SimLimits;
use cp_terrain::Terrain;

const TICKS: u64 = 512;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bench_checksum(extra: &[&str], tag: &str) -> String {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{tag}.json"));
    let _ = std::fs::remove_file(&out);
    let o = Command::new(env!("CARGO_BIN_EXE_cp_bench"))
        .current_dir(root())
        .args([
            "--ticks",
            &TICKS.to_string(),
            "--warmup",
            "0",
            "--seed",
            "1",
        ])
        .args(extra)
        .args(["--out", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap();
    json["checksum"].as_str().unwrap().to_string()
}

/// What `cp_desktop --map M --hour 0 --headless N --scene bare` does: `cp_app::load_sim` with a
/// scene that builds nothing (a plain start is a new game and adds the entry highway, which the
/// bench does not build), then N ticks.
fn client_checksum(map: &str) -> String {
    let terrain = Terrain::load(&root().join("assets/maps").join(map)).unwrap();
    let mut sim = cp_app::build_sim(terrain, SimLimits::desktop(), 0.0, false, Some("bare"));
    for _ in 0..TICKS {
        sim.tick_once();
    }
    format!("{:016x}", sim.checksum())
}

#[test]
fn bench_map_run_matches_the_client_setup_on_every_map() {
    for map in ["flatlands", "river_valley"] {
        assert_eq!(
            bench_checksum(&["--map", map], &format!("map-{map}")),
            client_checksum(map),
            "{map}: bench and client start from different sim states"
        );
    }
}

#[test]
fn map_is_actually_attached_to_the_sim() {
    // Without the fix `--map` only timed the load and the checksum equalled the map-less run.
    assert_ne!(
        bench_checksum(&["--map", "flatlands"], "with-map"),
        bench_checksum(&[], "no-map"),
        "--map did not change the sim state (terrain not attached?)"
    );
}

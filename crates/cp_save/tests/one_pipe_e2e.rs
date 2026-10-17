//! T-101: one pipe for water and sewage. Removing `sewage_pipe` from the content changes no
//! saved city: the old fixtures load (only the content hash differs) and, 128 ticks later,
//! every building has the same water, sewage and power flags as it had with the old content
//! (the digests were recorded with `sewage_pipe.ron` still in the content).

use std::path::Path;
use std::sync::Arc;

use cp_building::{BuildingFlags, BuildingGrid};
use cp_content::ContentDb;
use cp_core::SimLimits;
use cp_save::*;
use cp_sim::SimRunner;
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Digest of (building id, water/sewage/power flags) over all buildings, FNV-1a.
fn flags_digest(sim: &SimRunner) -> (u64, usize) {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut fed = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    let mut n = 0;
    let mask = BuildingFlags::NO_WATER | BuildingFlags::NO_SEWAGE | BuildingFlags::NO_POWER;
    for (id, b) in sim.world().resource::<BuildingGrid>().iter() {
        for byte in format!("{id:?}").bytes() {
            fed(byte);
        }
        fed(b.flags.0 & mask);
        n += 1;
    }
    (h, n)
}

fn after_128(name: &str) -> (u64, usize) {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves").join(name)).unwrap();
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let hash = content.content_hash();
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(content);
    let (_, warnings) = load_into_runner(&mut sim, &bytes, hash).unwrap();
    // The content differs from the one the file was saved with, and v17-basic was written on
    // another build of the map (as before this change); no definition may be unknown.
    assert!(
        warnings.iter().all(|w| matches!(
            w,
            LoadWarning::ContentHashMismatch { .. } | LoadWarning::MapMismatch { .. }
        )),
        "{name}: {warnings:?}"
    );
    for _ in 0..128 {
        sim.tick_once();
    }
    flags_digest(&sim)
}

#[test]
fn old_fixtures_keep_their_water_sewage_and_power_flags_without_the_sewage_pipe() {
    let got = [after_128("m1-small.sav"), after_128("v17-basic.sav")];
    // (digest, buildings) recorded with the old content; m1-small re-recorded for BUG-047 (zone layout v2).
    let want: [(u64, usize); 2] = [(2849559857892872563, 196), (18166537202897433253, 43)];
    assert_eq!(got, want);
}

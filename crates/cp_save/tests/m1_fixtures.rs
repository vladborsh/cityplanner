//! T-046: the two M1 fixtures (`gen_m1_fixtures`): a city of about 1900 residents built by
//! `grid_city`, and the same kind of city in a jam, with 700 cars and 150 pedestrians on the
//! road and a building demolished on pause right before the save (`BuildingGrid.removed` not
//! empty). Both load without warnings, migrate to the current format and save back to the same
//! bytes, and load + 4096
//! ticks gives the recorded checksum. They were made with BUG-022 in (routine trips are routed
//! by the traffic domain); the jam's 850 trips are sent by `scenario::send_citizens` so the
//! jam does not depend on the time of day.
//!
//! **T-066 (save v17).** The pinned checksums were re-recorded when the agents gained the
//! re-route fields: the checksum hashes the serialized agents, so every value moves with the
//! encoding. The behaviour did not change: hashed with the agents in the v16 layout, the
//! old values (`0x169a_6df5_9eee_92be` / `0xa0fd_aeed_3448_248a` for m1-small,
//! `0xedcc_aa46_d0b4_49a5` / `0x2513_97bf_beca_a44a` for m1-traffic) reproduce exactly.
//!
//! **T-078.** Re-recorded again: `highway_1w_1l/2l/4l` sort between the old road types, so the
//! dense road ids of `highway_1w_3l`, `large_6l` and `medium_4l` moved and the checksum hashes
//! them. The behaviour did not change: loaded against the content without the three new
//! highways, m1-small gives the previous `0xc6b6_08b5_17a9_0b18` at load.

//! **T-129 (save v20).** Re-recorded again: the checksum hashes the serialized economy state,
//! which gained the loans and the report's loan payments (two more bytes with no loans); the
//! behaviour did not change.
//!
//! **T-135 (save v22).** Re-recorded again: the checksum hashes the citizen grid (two more
//! fields per citizen, the `dead` table) and the new service state and coverage; the behaviour
//! did not change.
//!
//! **T-106 (save v19).** Re-recorded again: the checksum hashes the serialized buildings, and a
//! building now carries `footprint: Footprint::Lot(..)` and `access: Some(..)` (one more byte
//! each). The behaviour did not change: `save_v19.rs` loads these two files and `v18-basic` and
//! checks the building flags, the residents and the households after 256 ticks against the
//! values recorded before the type change.

//! **T-109.** Re-recorded again: `water_tower` sorts between `water_pump_station` and
//! `wind_turbine`, so the dense building ids of everything after it moved by one and the
//! checksum hashes them (the state at load); and a wind turbine now has no jobs (it is a
//! free-standing type), so the residents who worked at one lose that job and the state after
//! 4096 ticks moves. Checked: with the committed content of before this change (no tower, the
//! turbine with its one job) all four old values reproduce exactly.

//! **T-110.** Re-recorded again, ids only: `sewage_treatment_plant` sorts between
//! `sewage_outflow_station` and `water_pump_station`, so the dense building ids after it moved
//! by one. Checked: with the plant's two files taken out of the content, all four values of the
//! T-109 header above reproduce exactly (and the bench-10k baseline too).

//! **T-139.** Re-recorded again, all four values: the new clinic and hospital are in the content
//! hash (state at load), and the sick, the cured and the dead of the health row move the state
//! after 4096 ticks.
//!
//! **T-138.** Re-recorded again, the two "load + 4096 ticks" values only: citizens now age, die and
//! have children once per game day, so the state after a day moves. The state at load is
//! unchanged (no serialized type changed).
//!
//! **T-131 (save v21).** Re-recorded again: the sim checksum has a new part, the district map
//! and records (empty in these files), so every value moves. The behaviour did not change: the
//! districts are empty and nothing reads them yet; `save_v21.rs` runs the v20 fixture on to the
//! residents recorded on the v20 code.

use std::path::Path;
use std::sync::Arc;

use cp_building::BuildingGrid;
use cp_citizen::{CitizenGrid, TripBook};
use cp_content::ContentDb;
use cp_core::SimLimits;
use cp_save::*;
use cp_sim::SimRunner;
use cp_terrain::Terrain;
use cp_traffic::Agents;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(ROOT).join("../../fixtures/saves").join(name)).unwrap()
}

fn env() -> (Arc<Terrain>, Arc<ContentDb>) {
    (
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap()),
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap()),
    )
}

fn only_hash_mismatch(w: &[cp_save::LoadWarning]) -> bool {
    w.iter()
        .all(|w| matches!(w, cp_save::LoadWarning::ContentHashMismatch { .. }))
}

/// A runner with the map and content, the file loaded into it.
fn loaded(bytes: &[u8]) -> SimRunner {
    let (terrain, content) = env();
    let hash = content.content_hash();
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(content);
    let (_, warnings) = load_into_runner(&mut sim, bytes, hash).unwrap();
    // New content changes the content hash; any other warning is a problem.
    assert!(only_hash_mismatch(&warnings), "{warnings:?}");
    sim
}

/// Format, map identity, and a round trip: the v16 file migrates to the current format (v17,
/// T-066), which saves and reloads to the very same bytes.
fn check_file(name: &str) {
    let bytes = fixture(name);
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 16, "made by the v16 writer");
    assert_eq!(h.map_id, "flatlands");
    let (terrain, content) = env();
    assert_eq!(h.map_hash, Some(terrain.map_hash()));
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(only_hash_mismatch(&l.warnings), "{:?}", l.warnings);
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(
        l2.data, l.data,
        "{name}: migrate -> save -> load is lossless"
    );
    assert_eq!(
        save_with_map(&l2.data, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        again,
        "{name}: save -> load -> save is byte for byte"
    );
}

fn after_4096(sim: &mut SimRunner) -> u64 {
    for _ in 0..4096 {
        sim.tick_once();
    }
    sim.checksum()
}

#[test]
fn m1_small_round_trips_and_runs_to_the_recorded_checksum() {
    check_file("m1-small.sav");
    let mut sim = loaded(&fixture("m1-small.sav"));
    let people = sim.world().resource::<CitizenGrid>().citizen_count();
    assert!((1800..=2100).contains(&people), "{people} residents");
    assert_eq!(sim.checksum(), 0xa1b7d9eaff5fe281, "state at load");
    assert_eq!(
        after_4096(&mut sim),
        0x96359a1297efe394,
        "load + 4096 ticks"
    );
}

#[test]
fn m1_traffic_round_trips_and_its_trips_go_on() {
    check_file("m1-traffic.sav");
    let mut sim = loaded(&fixture("m1-traffic.sav"));
    {
        let a = sim.world().resource::<Agents>();
        assert!(a.vehicle_count() >= 600, "{} cars", a.vehicle_count());
        assert!(
            a.pedestrian_count() >= 100,
            "{} pedestrians",
            a.pedestrian_count()
        );
    }
    let removed = sim.world().resource::<BuildingGrid>().removed().to_vec();
    assert_eq!(
        removed.len(),
        1,
        "a demolished building is waiting to be purged"
    );
    let trips_at_load = sim.world().resource::<TripBook>().len();
    assert!(trips_at_load >= 800, "{trips_at_load} trips in flight");
    assert_eq!(sim.checksum(), 0xdea816473e53a8ea, "state at load");

    // The first tick drains the removed list; the rest of the jam keeps moving and arriving.
    sim.tick_once();
    assert!(sim.world().resource::<BuildingGrid>().removed().is_empty());
    for _ in 0..4095 {
        sim.tick_once();
    }
    assert_eq!(sim.checksum(), 0x1f7ef789597531af, "load + 4096 ticks");
    let trips_after = sim.world().resource::<TripBook>().len();
    assert!(
        trips_after < trips_at_load,
        "the jam clears: {trips_at_load} -> {trips_after} trips in flight"
    );
}

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

//! **BUG-052.** Re-recorded again, the two "load + 4096 ticks" values only (state at load
//! unchanged): the entry highways no longer cost upkeep, streets cost and cost to keep 40 % less,
//! the water tower and wind turbine cost 40 % less to keep and the other service buildings 22 %
//! more, so the treasury differs from the first booked day on.
//!
//! **T-170 (save v37).** Re-recorded again, all four values: the checksum hashes the week report
//! and the goods trucks, whose serialized shape changed (export line, `Endpoint`); the behaviour
//! of these cities is unchanged (they have no `Outside` node with a road to trade through).
//!
//! **T-187.** Re-recorded again, the two "load + 4096 ticks" values only (state at load
//! unchanged): `tax_household` x2.4 and `tax_job` x1.8 in balance.ron, so the treasury differs
//! from the first booked day on.
//!
//! **T-179.** Re-recorded again, the two "load + 4096 ticks" values only (state at load unchanged): tax
//! income and upkeep are booked every game day instead of at the week boundary, so the treasury
//! (and with it the checksum) differs from the first day on.
//!
//! **T-148.** Re-recorded again, the two "load + 4096 ticks" values only: the balance pass changes the
//! garbage, sickness and service numbers, so the state after a day moves.
//!
//! **T-146.** Re-recorded again, the two "load + 4096 ticks" values only (state at load unchanged): land value now counts service coverage and
//! garbage, homes need services for levels above 2, and garbage and dead count towards
//! abandonment, so the state moves from the first city pass.
//!
//! **T-140.** Re-recorded again, all four values: the cemetery and the crematorium are in the
//! content hash (state at load) and the dead are collected; the behaviour is otherwise unchanged.
//!
//! **T-139.** Re-recorded again, all four values: the new clinic and hospital are in the content
//! hash (state at load), and the sick, the cured and the dead of the health row move the state
//! after 4096 ticks.
//!
//! **T-138.** Re-recorded again, the two "load + 4096 ticks" values only: citizens now age, die and
//! have children once per game day, so the state after a day moves. The state at load is
//! unchanged (no serialized type changed).
//!
//! **T-208.** Re-recorded again, all four values: the 24 chain buildings and the new service move the dense building ids.
//!
//! **T-205 (save v41).** Re-recorded again, all four values: a fixture older than v41 gets its
//! ore and oil deposits filled from the map at load, and the checksum hashes them. The behaviour
//! did not change.
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
    assert_eq!(sim.checksum(), 0x370323d1071302d2, "state at load");
    assert_eq!(
        after_4096(&mut sim),
        0x1f125df00844bf97,
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
    assert_eq!(sim.checksum(), 0x8b1c3efabd1b65f5, "state at load");

    // The first tick drains the removed list; the rest of the jam keeps moving and arriving.
    sim.tick_once();
    assert!(sim.world().resource::<BuildingGrid>().removed().is_empty());
    for _ in 0..4095 {
        sim.tick_once();
    }
    assert_eq!(sim.checksum(), 0x4e108b5e58e3e179, "load + 4096 ticks");
    let trips_after = sim.world().resource::<TripBook>().len();
    assert!(
        trips_after < trips_at_load,
        "the jam clears: {trips_at_load} -> {trips_after} trips in flight"
    );
}

/// T-139: the sick-by-home index is derived, not saved; a save taken mid-day with sick citizens
/// loads and runs on to the very state of the run that never stopped.
#[test]
fn a_save_with_sick_citizens_resumes_like_the_continuous_run() {
    let (_, content) = env();
    let mut a = loaded(&fixture("m1-small.sav"));
    for _ in 0..1500 {
        a.tick_once();
    }
    let now = a.tick();
    {
        let mut g = a.world_mut().resource_mut::<CitizenGrid>();
        g.ensure_derived();
        let ids: Vec<_> = g.citizens().map(|(id, _)| id).step_by(3).collect();
        assert!(ids.len() > 100);
        for id in ids {
            g.set_sick(id, Some(now));
        }
    }
    let bytes = save_runner(&a, content.content_hash(), "flatlands").unwrap();
    let mut b = loaded(&bytes);
    assert_eq!(a.checksum(), b.checksum(), "state at load");
    for _ in 0..256 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum(), "load + 256 ticks");
    let sick = |s: &SimRunner| {
        s.world()
            .resource::<CitizenGrid>()
            .citizens()
            .filter(|(_, c)| c.sick_since.is_some())
            .count()
    };
    assert_eq!(sick(&a), sick(&b));
}

/// T-172: new roads sort between the old ones, so the dense road indices of the content moved;
/// an old save names its roads and still gets the same types.
#[test]
fn an_old_save_keeps_its_road_types_when_roads_are_added() {
    let (_, content) = env();
    let l = load_with_map(&fixture("m1-small.sav"), content.content_hash(), None).unwrap();
    let saved = &l.data.def_names.roads;
    assert!(saved.iter().any(|n| n == "basic_2l"));
    assert!(
        !saved
            .iter()
            .any(|n| n.contains("1w_2l") && n.starts_with("basic"))
    );
    let sim = loaded(&fixture("m1-small.sav"));
    let net = sim.world().resource::<cp_net::NetGraph>();
    let mut used = 0;
    for (_, s) in net.segments() {
        let name = &content.road(s.road).id;
        assert!(saved.contains(name), "{name} was not in the save");
        used += 1;
    }
    assert!(used > 0);
}

//! T-046: the two M1 fixtures (`gen_m1_fixtures`): a city of about 1900 residents built by
//! `grid_city`, and the same kind of city in a jam, with 700 cars and 150 pedestrians on the
//! road and a building demolished on pause right before the save (`BuildingGrid.removed` not
//! empty). Both load without warnings, save back to the very same bytes, and load + 4096
//! ticks gives the recorded checksum. They were made with BUG-022 in (routine trips are routed
//! by the traffic domain); the jam's 850 trips are sent by `scenario::send_citizens` so the
//! jam does not depend on the time of day.

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

/// A runner with the map and content, the file loaded into it.
fn loaded(bytes: &[u8]) -> SimRunner {
    let (terrain, content) = env();
    let hash = content.content_hash();
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(content);
    let (_, warnings) = load_into_runner(&mut sim, bytes, hash).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    sim
}

/// Format, map identity and a byte-for-byte round trip.
fn check_file(name: &str) {
    let bytes = fixture(name);
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 16, "made by the v16 writer");
    assert_eq!(h.map_id, "flatlands");
    let (terrain, content) = env();
    assert_eq!(h.map_hash, Some(terrain.map_hash()));
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        again, bytes,
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
    assert_eq!(sim.checksum(), 0x169a_6df5_9eee_92be, "state at load");
    assert_eq!(
        after_4096(&mut sim),
        0xa0fd_aeed_3448_248a,
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
    assert_eq!(sim.checksum(), 0xedcc_aa46_d0b4_49a5, "state at load");

    // The first tick drains the removed list; the rest of the jam keeps moving and arriving.
    sim.tick_once();
    assert!(sim.world().resource::<BuildingGrid>().removed().is_empty());
    for _ in 0..4095 {
        sim.tick_once();
    }
    assert_eq!(sim.checksum(), 0x2513_97bf_beca_a44a, "load + 4096 ticks");
    let trips_after = sim.world().resource::<TripBook>().len();
    assert!(
        trips_after < trips_at_load,
        "the jam clears: {trips_at_load} -> {trips_after} trips in flight"
    );
}

//! T-046: the two M1 fixtures (`gen_m1_fixtures`): a city of about 2700 residents built by
//! `grid_city`, and the same kind of city in a jam, with some 600 cars and 450 pedestrians on the
//! road and a building demolished on pause right before the save (`BuildingGrid.removed` not
//! empty). Both load without warnings and save back to the same bytes, and load + a day (2048 ticks since T-301)
//! gives the recorded checksum. The jam's 850 trips are sent by `scenario::send_citizens` so it
//! does not depend on the time of day.
//!
//! Old saves are not migrated during development (T-275, AGENTS.md): when the format changes,
//! `gen_m1_fixtures` writes the two files again with the current writer and prints the checksums
//! to pin here. Made last for format 59 (T-301: the game day is 2048 ticks; format 58 was T-290: the land grids carry the garbage nuisance; format 57 was T-275: roads are whole cells wide, the 96 m lattice of
//! `grid_city`). BUG-090 moved the two `load + a day (2048 ticks since T-301)` pins (state at load unchanged): a car
//! that has stood at the stop line with its lane change blocked now turns from its own lane or
//! goes another way instead of waiting to be removed as stuck, and a car starting near its
//! junction starts in the lane it needs.

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

/// Format, map identity, and a round trip: the file saves and reloads to the very same bytes.
fn check_file(name: &str) {
    let bytes = fixture(name);
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(
        h.format_version, FORMAT_VERSION,
        "made by the current writer"
    );
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
    assert_eq!(l2.data, l.data, "{name}: load -> save -> load is lossless");
    assert_eq!(
        save_with_map(&l2.data, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        again,
        "{name}: save -> load -> save is byte for byte"
    );
}

fn after_a_day(sim: &mut SimRunner) -> u64 {
    for _ in 0..cp_core::time::TICKS_PER_DAY {
        sim.tick_once();
    }
    sim.checksum()
}

#[test]
fn m1_small_round_trips_and_runs_to_the_recorded_checksum() {
    check_file("m1-small.sav");
    let mut sim = loaded(&fixture("m1-small.sav"));
    let people = sim.world().resource::<CitizenGrid>().citizen_count();
    // T-300 + T-301 (format 60): the fixture holds 2479 residents.
    assert!((2400..=3000).contains(&people), "{people} residents");
    // T-300b: re-recorded (was 0xb1ab96f8db47fe52), format 61 and the water dirt.
    // T-305: re-recorded (was 0x422e143067e61039), 70 km/h avenues and reach by time.
    assert_eq!(sim.checksum(), 0x50e9581a7e49d1a5, "state at load"); // T-303a: re-recorded (was 0xf1dee787e8fe8b5f), format 62, the signs on the nodes. T-308: re-recorded, the new utility content changes the content hash.
    // T-280: re-recorded (was 0xa0961668822b2784 since T-282): only 15 % of car trips and
    // 25 % of goods trucks take a vehicle, the rest run abstractly, so the day after the load
    // has other cars on the road.
    // T-282: re-recorded after the balance pass (taxes x5, escalation 5x slower, services 3x, cheaper basic services).
    // T-270: re-recorded, the movement clock is 0.4 s a tick (§2 v1.2): agents cover a fifth
    // of the ground per tick, so the day after the load runs differently.
    assert_eq!(
        after_a_day(&mut sim),
        0x3525f19247d6d612, // T-305: re-recorded (was 0x1172029647ed4c94), 70 km/h avenues and reach by time. T-303a: re-recorded (was 0x952c338864d38fa6), format 62. T-300b: re-recorded (was 0x46c86fe1c3fd6e4d), the water dirt. T-301: re-recorded, the day is 2048 ticks and the test runs a day (was 0x68f162061e8c655). T-295: re-recorded (balance: taxes x1.6, fire x2, landfill 3500, police 2000, deaths from 50). BUG-095: re-recorded, services make no shop goods and are no industry in the demand.
        "load + a day"
    );
}

#[test]
fn m1_traffic_round_trips_and_its_trips_go_on() {
    check_file("m1-traffic.sav");
    let mut sim = loaded(&fixture("m1-traffic.sav"));
    {
        let a = sim.world().resource::<Agents>();
        // T-290: 20 (was 500): since T-280 only 15 % of the car trips take a vehicle, so the
        // jam the generator stops at holds ~30 cars (the rest of the trips run abstractly).
        assert!(a.vehicle_count() >= 20, "{} cars", a.vehicle_count());
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
    let done_at_load = sim.world().resource::<TripBook>().completed;
    // T-290: 300 (was 800): the trips that run abstractly (T-280) are over by the time cars
    // and pedestrians are on the road; ~330 are in flight when the generator stops.
    assert!(trips_at_load >= 300, "{trips_at_load} trips in flight");
    // T-300b: re-recorded (was 0xc0655f1fa1b9d97b), format 61 and the water dirt.
    // T-303a: re-recorded (was 0x24cd57bc0147d1f2), format 62, the signs on the nodes.
    // T-305: re-recorded (was 0x7651f529a497cfac), 70 km/h avenues and reach by time.
    assert_eq!(sim.checksum(), 0x10c365bdd165fb4e, "state at load");

    // The first tick drains the removed list; the rest of the jam keeps moving and arriving.
    sim.tick_once();
    assert!(sim.world().resource::<BuildingGrid>().removed().is_empty());
    for _ in 0..cp_core::time::TICKS_PER_DAY - 1 {
        sim.tick_once();
    }
    // BUG-095: re-recorded (was 0x6157ca2ba25af219), services make no shop goods and are no
    // industry in the demand.
    // T-280: re-recorded (was 0x35693ab6f390ce23 since T-270): the trips planned after the
    // load mostly run abstractly (car_visible_share 0.15); the jam saved with the file drives.
    // T-270: re-recorded, the movement clock is 0.4 s a tick (§2 v1.2).
    // T-295: re-recorded (was 0x54a58aeb68367963), the balance pass.
    // T-301: re-recorded, a game day is 2048 ticks (was 0x63538024f0946108 after 4096).
    // T-300b: re-recorded (was 0xcd9ffc8eeeb52aaa), the water dirt.
    // T-303a: re-recorded (was 0xecaa77b6c0c32b93), format 62.
    // T-305: re-recorded (was 0x559a6ce3e0faabbd), 70 km/h avenues and reach by time.
    assert_eq!(sim.checksum(), 0xbb80f8c8a501fac8, "load + a day");
    // T-290: the trips of the saved jam arrive (completed grows by hundreds); the number in flight
    // no longer falls, the routine plans new ones all day (most of them abstract, T-280).
    let done_after = sim.world().resource::<TripBook>().completed;
    assert!(
        done_after >= done_at_load + 200,
        "the trips go on: {done_at_load} -> {done_after} completed"
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

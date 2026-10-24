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
    // T-300 + T-301 (format 60): the fixture held 2479 residents. T-318b (format 71): 2131,
    // move-in follows the home index and unhappy homes lose households.
    assert!((2000..=3000).contains(&people), "{people} residents");
    // T-300b: re-recorded (was 0xb1ab96f8db47fe52), format 61 and the water dirt.
    // T-305: re-recorded (was 0x422e143067e61039), 70 km/h avenues and reach by time.
    // T-302b: re-recorded (was 0x50e9581a7e49d1a5), format 63, the tram content.
    // T-302c: re-recorded (was 0x09461c008868d8df), format 64, the trolleybus content; regenerated on T-311/T-306.
    // T-315: re-recorded (was 0x3af4471b47254980), format 65, routes by measured lane delay.
    // T-314: re-recorded (was 0x7ff4e347d060a49f), format 66 (the automatic car share, off).
    // T-318a: re-recorded (was 0xc240e8eb90227496), format 67, the homes' contentment is saved.
    // T-324: re-recorded (was 0xe1466aa8b57adf30), format 68, the corruption state is saved.
    // T-318b: re-recorded (was 0x2327d9f958eaacfd), format 71, trip start ticks and commutes.
    assert_eq!(sim.checksum(), 0xf236d9837cf90f93, "state at load"); // T-332: re-recorded (was 0x59c1deab54f3bc14), format 92, fixtures regenerated on T-333. // T-321: re-recorded (was 0xbb2592b12739c3c8), format 91, hotels. T-322: re-recorded (was 0x691c3d8fc3d4b610), format 88, bicycles. T-320: re-recorded (was 0x4ecafd4b4b9e2f4d), format 87, the intercity market. T-327: re-recorded (was 0xeab1a0a007720560), format 84, the institutions. T-319a: re-recorded (was 0x52ab476c4d87e8aa), format 75, the service budgets a resource of their own (regenerated; the sim at 100 % is unchanged: every budget factor is exactly 1). T-326: re-recorded (was 0x9a6cb34bf9dab902), format 74, the scandals. T-325: re-recorded (was 0x1c7c0c4d92f1e0d8), format 72, grey deals. T-313: re-recorded (was 0x843f183090b1d8f7), format 69, lane rules. T-303a: re-recorded (was 0xf1dee787e8fe8b5f), format 62, the signs on the nodes. T-308: re-recorded, the new utility content changes the content hash. // T-323a: re-recorded (was 0x4863c217bbf9c58e), format 79, Segment.trees. // T-331a: re-recorded (was 0xfff67dc9aef8f5b2), format 83, parking content and Budgets of 17 kinds. // T-331b1: re-recorded (was 0x79a3cf7aee19ba59), format 85, cars park in reserved places. // T-331b2: re-recorded (was 0x5c20d2aa59993c73), format 86, the walk to the parked car.
    // T-280: re-recorded (was 0xa0961668822b2784 since T-282): only 15 % of car trips and
    // 25 % of goods trucks take a vehicle, the rest run abstractly, so the day after the load
    // has other cars on the road.
    // T-282: re-recorded after the balance pass (taxes x5, escalation 5x slower, services 3x, cheaper basic services).
    // T-270: re-recorded, the movement clock is 0.4 s a tick (§2 v1.2): agents cover a fifth
    // of the ground per tick, so the day after the load runs differently.
    assert_eq!(
        after_a_day(&mut sim),
        0xee8c871bc67ee0cc, // T-332: re-recorded (was 0xa69b38a8620bb792), format 92, fixtures regenerated on T-333. // T-333: re-recorded (was 0xfb268212a53b2603), the zebra rule, no format change. // T-321: re-recorded (was 0x66510b43d6275052), format 91, hotels. T-322: re-recorded (was 0x00743a285cf892f6), format 88, bicycles. T-320: re-recorded (was 0x9b858febf544dfa1), format 87, the intercity market. T-327: re-recorded (was 0x4d5e6267b6cc5c5b), format 84, the institutions. T-319a: re-recorded (was 0x3523ec2aa595fa79), format 75, the service budgets a resource of their own (regenerated; the sim at 100 % is unchanged: every budget factor is exactly 1). T-326: re-recorded (was 0x031d222b3121f753), format 74, the scandals. T-329a: re-recorded (was 0x749ec09458fd9988), walkers 1.19-1.61 m/s by citizen hash. T-325: re-recorded (was 0x185e48a7f33b42fe), format 72, grey deals and the corruption update. T-318b: re-recorded (was 0xef8c3714acf8a86b), format 71, the work and commute factors, move-in by contentment, move-out and wellbeing. T-313: re-recorded (was 0x69a2707ad7c5ea29), format 69, lane rules. // T-324: re-recorded (was 0x2217973baef189aa), format 68, trust and the daily corruption update. T-318a: re-recorded (was 0x5a953b38964f73b5), format 67, the contentment pass. T-314: re-recorded (was 0x6aa91e835db6a43d), format 66, free-flowing cars take one step a tick. T-315: re-recorded (was 0xa2e6dc91e00d114c), format 65, routes by measured lane delay and traffic re-routes. T-302c: re-recorded (was 0xf3b290b7e23c1445), format 64, the trolleybus content, the fixture regenerated on T-311/T-306. BUG-113: re-recorded (was 0xbda760a646dccb6b), garbage trucks go for half a problem and book the landfill's day. T-311: re-recorded (was 0x474144ecf293bb70), half the arrivals (MOVE_IN_ONE_IN 4) and the trips of the departed cancelled at once. T-302b: re-recorded (was 0x3525f19247d6d612), format 63, the tram content. T-305: re-recorded (was 0x1172029647ed4c94), 70 km/h avenues and reach by time. T-303a: re-recorded (was 0x952c338864d38fa6), format 62. T-300b: re-recorded (was 0x46c86fe1c3fd6e4d), the water dirt. T-301: re-recorded, the day is 2048 ticks and the test runs a day (was 0x68f162061e8c655). T-295: re-recorded (balance: taxes x1.6, fire x2, landfill 3500, police 2000, deaths from 50). BUG-095: re-recorded, services make no shop goods and are no industry in the demand. // T-323a: re-recorded (was 0x2dc69a0c9e0ce6fd), format 79, Segment.trees. // T-331a: re-recorded (was 0x4316da16e7982e39), format 83, parking content and Budgets of 17 kinds. // T-331b1: re-recorded (was 0xf5fe4433de9597ed), format 85, cars park in reserved places. // T-331b2: re-recorded (was 0x2bc14b269fb46cf9), format 86, the walk to the parked car.
        "load + a day"
    );
    // T-314: a town of 2.5k residents stays under the car budget: with visible_share_auto on
    // every car trip would drive (it is off in balance.ron until the rebalancing).
    let v = sim.world().resource::<Agents>().visible;
    assert_eq!(v.share, 1.0, "{v:?}");
    assert!(v.expected_cars > 0.0, "{v:?}");
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
    // T-302b: re-recorded (was 0x10c365bdd165fb4e), format 63, the tram content.
    // T-302c: re-recorded (was 0x3b0205fb9fe1ffc7), format 64, regenerated on T-311/T-306.
    // T-315: re-recorded (was 0x9ea3605c8fbfffd3), format 65, routes by measured lane delay.
    // T-314: re-recorded (was 0x99a8a4566e60d889), format 66 (the automatic car share, off).
    // T-318a: re-recorded (was 0x45b2f780da9f32c7), format 67, the homes' contentment is saved.
    // T-324: re-recorded (was 0x7abb7a47e5410231), format 68, the corruption state is saved.
    // T-318b: re-recorded (was 0xb32037d8fec814ab), format 71, trip start ticks and commutes.
    // T-325: re-recorded (was 0x83a43f32f177e8cd), format 72, grey deals.
    // T-326: re-recorded (was 0x1cd996d17d1d4369), format 74, the scandals.
    // T-319a: re-recorded (was 0x31758a050c798ea3), format 75, the service budgets a resource of their own (regenerated; the sim at 100 % is unchanged: every budget factor is exactly 1).
    assert_eq!(sim.checksum(), 0xaf7e496b4c8b41de, "state at load"); // T-332: re-recorded (was 0x1a2a7b8a356805bf), format 92, fixtures regenerated on T-333. // T-321: re-recorded (was 0x31065fbabdd7f47c), format 91, hotels. T-322: re-recorded (was 0x73c162617e6f8fb3), format 88, bicycles. T-320: re-recorded (was 0xfca6cada49a9202f), format 87, the intercity market. T-327: re-recorded (was 0xfca52517b90ee43f), format 84, the institutions. T-323a: re-recorded (was 0x4c9b758adb4135df), format 79, Segment.trees. // T-331a: re-recorded (was 0x235d4d3828d3e8c5), format 83, parking content and Budgets of 17 kinds. // T-331b1: re-recorded (was 0x6f2cd1ad05297ab5), format 85, cars park in reserved places. // T-331b2: re-recorded (was 0x4f8f20baf1b2c64d), format 86, the walk to the parked car.

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
    // T-302b: re-recorded (was 0xbb80f8c8a501fac8), format 63, the tram content.
    // BUG-113: re-recorded (was 0xabf544df409b48f7), garbage trucks go for half a problem and book the landfill's day.
    // T-302c: re-recorded (was 0xfa28152f0738ac4f), format 64, regenerated on T-311/T-306.
    // T-315: re-recorded (was 0x67bbfe3cc3121206), format 65, routes by measured lane delay and traffic re-routes.
    // T-314: re-recorded (was 0xa4e6eef5d8efd9eb), format 66, free-flowing cars take one step a tick.
    // T-318a: re-recorded (was 0x1e1dfb34abf3b58e), format 67, the contentment pass.
    // T-324: re-recorded (was 0xbaa59b6954cdbd4f), format 68, trust and the daily update.
    // T-318b: re-recorded (was 0x59e639e2f01f8194), format 71, the T-318b factors and effects.
    // T-325: re-recorded (was 0xb754511da09cbacb), format 72, grey deals and the corruption update.
    // T-329a: re-recorded (was 0x6405753f9c4a70ab), walkers 1.19-1.61 m/s by citizen hash.
    // T-326: re-recorded (was 0xecd140d9f407c6c6), format 74, the scandals.
    // T-319a: re-recorded (was 0xaaf2d5ebce1890a2), format 75, the service budgets a resource of their own (regenerated; the sim at 100 % is unchanged: every budget factor is exactly 1).
    assert_eq!(sim.checksum(), 0x79f28c55837e5423, "load + a day"); // T-332: re-recorded (was 0x327ee8c19673944e), format 92, fixtures regenerated on T-333. // T-333: re-recorded (was 0x0aab023eaf7c1921), the zebra rule, no format change. // T-321: re-recorded (was 0x68890ad3d5cbcec8), format 91, hotels. T-322: re-recorded (was 0xaa520708a46d3b77), format 88, bicycles. T-320: re-recorded (was 0x2f0094e914e62d35), format 87, the intercity market. T-327: re-recorded (was 0x47f333f83f8b5b52), format 84, the institutions. T-323a: re-recorded (was 0x101c0c61972a9f5e), format 79, Segment.trees. // T-331a: re-recorded (was 0xa4d41bf13772ec98), format 83, parking content and Budgets of 17 kinds. // T-331b1: re-recorded (was 0x6f6e00e3aebcd7d8), format 85, cars park in reserved places. // T-331b2: re-recorded (was 0x0de2236ba251fc2b), format 86, the walk to the parked car.
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

/// T-315: the lane delays the pathfinder reads, the hot lanes and a traffic re-route in flight
/// (the request, and the car's flag that says how to take the answer) are saved: a save taken
/// while cars wait for quicker routes loads and runs on to the state of the run that never
/// stopped.
#[test]
fn a_save_with_traffic_reroutes_in_flight_resumes_like_the_continuous_run() {
    let (_, content) = env();
    let mut a = loaded(&fixture("m1-traffic.sav"));
    let in_flight = |s: &SimRunner| {
        s.world()
            .resource::<Agents>()
            .vehicles()
            .filter(|(_, v)| v.kind & cp_traffic::KIND_TRAFFIC_REROUTE != 0)
            .count()
    };
    let mut found = false;
    for _ in 0..3000 {
        a.tick_once();
        if in_flight(&a) > 0 {
            found = true;
            break;
        }
    }
    assert!(found, "a traffic re-route was asked for");
    let bytes = save_runner(&a, content.content_hash(), "flatlands").unwrap();
    let mut b = loaded(&bytes);
    assert_eq!(a.checksum(), b.checksum(), "state at load");
    for _ in 0..256 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum(), "load + 256 ticks");
    let st = &a.world().resource::<Agents>().stats;
    assert!(st.traffic_rerouted + st.traffic_kept > 0, "{st:?}");
}

//! T-331b1 (§18.3): a car goes to a parking place chosen and reserved at route time, stands in it
//! (`Agents::parked`) and its owner walks on to the goal; no place within reach and the car
//! disappears at the door (`parked_abstract`, `NO_PARKING`).
//!
//! Towns are grown as in `agents_e2e`, the routine is switched off and single trips are injected
//! so the numbers are exact.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_citizen::{
    CitizenGrid, CitizenId, Location, ParkPlace, RoutineSettings, Trip, TripBook, TripMode,
    TripPurpose, TripState,
};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_traffic::{AgentSettings, Agents, ParkedCar, ParkingIndex};
use cp_zone::{ZoneCellRef, ZoneGrid};

/// The shipped content with every car trip taking a car (T-280's `car_visible_share` hides
/// most in the game): these tests are about the pools and the agents themselves.
fn content() -> Arc<ContentDb> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "ron") {
                let mut text = std::fs::read_to_string(&path).unwrap();
                if path.file_name().is_some_and(|n| n == "balance.ron") {
                    text = text
                        .lines()
                        .map(|l| {
                            if l.trim_start().starts_with("car_visible_share:") {
                                "    car_visible_share: 1.0,"
                            } else {
                                l
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                out.push((path.display().to_string(), text));
            }
        }
    }
    let mut src = Vec::new();
    walk(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")),
        &mut src,
    );
    let db = ContentDb::from_sources(src).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(db.balance().car_visible_share, 1.0);
    Arc::new(db)
}

/// An `n`-line lattice of 90 m streets of `road`; blocks are zoned in a repeating pattern.
fn town(
    content: &Arc<ContentDb>,
    seed: u64,
    lines: usize,
    road: &str,
    kinds: &[ZoneType],
) -> SimRunner {
    let road = content.road_id(road).unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(seed);
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(1.0);
    sim.set_content(Arc::clone(content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(2000.0 + x, 0.0, 2000.0 + z);
        for line in 0..lines {
            let o = line as f32 * 90.0;
            for piece in 0..lines - 1 {
                let a = piece as f32 * 90.0;
                for (from, to) in [(p(a, o), p(a + 90.0, o)), (p(o, a), p(o, a + 90.0))] {
                    g.add_segment(&def, road, CubicBezier3::line(from, to), Elevation::Ground)
                        .unwrap();
                }
            }
        }
    }
    sim.tick_once();
    {
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(id, b)| (id, b.cell_count())).collect();
        for (n, (id, cells)) in ids.into_iter().enumerate() {
            for cell in 0..cells {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    kinds[n % kinds.len()],
                );
            }
        }
    }
    sim
}

const MIX: [ZoneType; 4] = [
    ZoneType::ResHigh,
    ZoneType::ResHigh,
    ZoneType::Industrial,
    ZoneType::ComHigh,
];

/// Grows until `n` citizens live there, settles for two days, switches the routine off and
/// waits until nothing is travelling.
fn quiet_town(
    content: &Arc<ContentDb>,
    seed: u64,
    lines: usize,
    road: &str,
    n: usize,
) -> SimRunner {
    let mut sim = town(content, seed, lines, road, &MIX);
    while sim.world().resource::<CitizenGrid>().citizen_count() < n {
        sim.tick_once();
        assert!(
            sim.tick().0 < 20 * TICKS_PER_DAY,
            "town never reached {n} citizens"
        );
    }
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    sim.world_mut().resource_mut::<RoutineSettings>().decide = false;
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
        if sim.world().resource::<TripBook>().is_empty() {
            break;
        }
    }
    assert!(
        sim.world().resource::<TripBook>().is_empty(),
        "town is quiet"
    );
    sim
}

/// A citizen of the town at home, and a building other than their home with a yard.
fn pick(sim: &SimRunner) -> (CitizenId, BuildingId, BuildingId) {
    let grid = sim.world().resource::<CitizenGrid>();
    let index = sim.world().resource::<ParkingIndex>();
    let buildings = sim.world().resource::<BuildingGrid>();
    let (cid, home) = grid
        .citizens()
        .filter(|(_, c)| c.is_adult() && c.location == Location::Home)
        .filter_map(|(id, c)| c.home.map(|h| (id, h)))
        .next()
        .expect("an adult at home");
    // A goal 200 m or more away, so the trip is a drive and not a walk.
    let home_pos = buildings.get(home).unwrap().pos;
    let goal = buildings
        .iter()
        .filter(|(id, b)| {
            b.state == BuildingState::Active
                && *id != home
                && index.building(*id).free() > 0
                && b.pos.distance(home_pos) > 150.0
                && b.access().is_some()
        })
        .map(|(id, _)| id)
        .next()
        .expect("a goal with a yard");
    (cid, home, goal)
}

fn send(sim: &mut SimRunner, cid: CitizenId, from: BuildingId, to: BuildingId) {
    sim.world_mut().resource_mut::<AgentSettings>().mode_by_time = false;
    sim.world_mut()
        .resource_scope(|w, mut book: bevy_ecs::change_detection::Mut<TripBook>| {
            book.ensure_derived();
            book.add(Trip {
                citizen: cid,
                from,
                to,
                purpose: TripPurpose::Work,
                mode: TripMode::Car,
                state: TripState::Planned,
                started: cp_core::Tick(0),
                park: Default::default(),
            })
            .expect("a free citizen");
            w.resource_mut::<CitizenGrid>()
                .set_location(cid, Location::Moving);
        });
}

fn drain(sim: &mut SimRunner) {
    for _ in 0..(TICKS_PER_DAY / 2) {
        sim.tick_once();
        if sim.world().resource::<TripBook>().is_empty() {
            return;
        }
    }
    panic!("the trip never ended");
}

/// Fills the places of `place` with the cars of other citizens (the authoritative state the
/// index counts from).
fn fill(sim: &mut SimRunner, place: ParkPlace, n: u16, except: CitizenId) {
    let others: Vec<CitizenId> = sim
        .world()
        .resource::<CitizenGrid>()
        .citizens()
        .map(|(id, _)| id)
        .filter(|&id| id != except)
        .take(usize::from(n))
        .collect();
    assert_eq!(others.len(), usize::from(n), "enough citizens to fill with");
    let mut agents = sim.world_mut().resource_mut::<Agents>();
    for c in others {
        assert!(agents.parked.park(
            c,
            ParkedCar {
                place,
                since: cp_core::Tick(0)
            }
        ));
    }
    let cars: Vec<ParkPlace> = agents.parked.iter().map(|(_, c)| c.place).collect();
    sim.world_mut().resource_mut::<ParkingIndex>().sync(cars);
}

#[test]
fn a_car_parks_in_the_yard_of_its_goal_and_the_citizen_walks_in() {
    let content = content();
    let mut sim = quiet_town(&content, 5, 5, "basic_2l", 300);
    let (cid, home, goal) = pick(&sim);
    let before = sim.world().resource::<Agents>().stats.clone();
    send(&mut sim, cid, home, goal);
    drain(&mut sim);
    let agents = sim.world().resource::<Agents>();
    let car = agents.parked.get(cid).expect("the car is parked");
    assert_eq!(car.place, ParkPlace::Building(goal));
    assert_eq!(agents.stats.parked, before.parked + 1);
    assert_eq!(agents.stats.parked_abstract, before.parked_abstract);
    assert_eq!(
        sim.world()
            .resource::<CitizenGrid>()
            .citizen(cid)
            .unwrap()
            .location,
        Location::Work
    );
    assert!(
        !sim.world()
            .resource::<BuildingGrid>()
            .get(goal)
            .unwrap()
            .flags
            .has(BuildingFlags::NO_PARKING)
    );
}

#[test]
fn a_full_yard_sends_the_car_to_the_kerb_and_the_citizen_walks() {
    let content = content();
    let mut sim = quiet_town(&content, 5, 5, "basic_2l", 300);
    let (cid, home, goal) = pick(&sim);
    let cap = sim.world().resource::<ParkingIndex>().building(goal).free();
    fill(&mut sim, ParkPlace::Building(goal), cap, cid);
    let walked = sim.world().resource::<Agents>().stats.walked;
    let abstract_before = sim.world().resource::<Agents>().stats.parked_abstract;
    send(&mut sim, cid, home, goal);
    drain(&mut sim);
    let agents = sim.world().resource::<Agents>();
    let car = agents.parked.get(cid).expect("the car is parked");
    assert!(
        matches!(car.place, ParkPlace::Kerb { .. }),
        "{:?}",
        car.place
    );
    assert_eq!(agents.stats.walked, walked + 1, "the last leg is on foot");
    assert_eq!(agents.stats.parked_abstract, abstract_before);
}

#[test]
fn no_place_in_reach_parks_abstractly_and_flags_the_goal() {
    let content = content();
    // medium_4l has no kerb parking and the town has no lots: only the yards.
    let mut sim = quiet_town(&content, 5, 5, "medium_4l", 300);
    let (cid, home, goal) = pick(&sim);
    let cap = sim.world().resource::<ParkingIndex>().building(goal).free();
    fill(&mut sim, ParkPlace::Building(goal), cap, cid);
    let before = sim.world().resource::<Agents>().stats.parked_abstract;
    send(&mut sim, cid, home, goal);
    drain(&mut sim);
    let agents = sim.world().resource::<Agents>();
    assert!(agents.parked.get(cid).is_none());
    assert_eq!(agents.stats.parked_abstract, before + 1);
    assert!(
        sim.world()
            .resource::<BuildingGrid>()
            .get(goal)
            .unwrap()
            .flags
            .has(BuildingFlags::NO_PARKING)
    );
    assert_eq!(
        sim.world()
            .resource::<CitizenGrid>()
            .citizen(cid)
            .unwrap()
            .location,
        Location::Work
    );
}

#[test]
fn many_cars_park_and_the_index_counts_exactly_the_parked_cars() {
    let content = content();
    let mut sim = quiet_town(&content, 7, 5, "basic_2l", 300);
    sim.world_mut().resource_mut::<AgentSettings>().mode_by_time = false;
    let movers: Vec<(CitizenId, BuildingId)> = {
        let grid = sim.world().resource::<CitizenGrid>();
        grid.citizens()
            .filter(|(_, c)| c.is_adult() && c.location == Location::Home)
            .filter_map(|(id, c)| c.home.map(|h| (id, h)))
            .take(60)
            .collect()
    };
    let goals: Vec<BuildingId> = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active && b.access().is_some())
        .map(|(id, _)| id)
        .collect();
    for (i, &(cid, home)) in movers.iter().enumerate() {
        let mut to = goals[(i * 7 + 3) % goals.len()];
        if to == home {
            to = goals[(i * 7 + 4) % goals.len()];
        }
        send(&mut sim, cid, home, to);
    }
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
        if sim.world().resource::<TripBook>().is_empty() {
            break;
        }
    }
    assert!(sim.world().resource::<TripBook>().is_empty());
    // One more index pass, then every counted car is a parked one.
    for _ in 0..70 {
        sim.tick_once();
    }
    let agents = sim.world().resource::<Agents>();
    let index = sim.world().resource::<ParkingIndex>();
    let used = index.kerb_total().1 + index.building_total().1;
    assert_eq!(used as usize, agents.parked.len());
    assert!(agents.parked.len() > 10, "{} parked", agents.parked.len());
    let mut seen: BTreeMap<ParkPlace, u32> = BTreeMap::new();
    for (_, c) in agents.parked.iter() {
        assert!(index.exists(c.place));
        *seen.entry(c.place).or_default() += 1;
    }
    for (place, n) in seen {
        if matches!(place, ParkPlace::Kerb { .. }) {
            assert_eq!(n, 1, "{place:?} holds one car");
        }
    }
}

#[test]
fn the_owner_walks_to_the_parked_car_before_driving_home() {
    let content = content();
    let mut sim = quiet_town(&content, 5, 5, "basic_2l", 300);
    let (cid, home, goal) = pick(&sim);
    // The goal's yard is full, so the car stands in the street and the way back starts on foot.
    let free = sim.world().resource::<ParkingIndex>().building(goal).free();
    fill(&mut sim, ParkPlace::Building(goal), free, cid);
    send(&mut sim, cid, home, goal);
    drain(&mut sim);
    let car = *sim
        .world()
        .resource::<Agents>()
        .parked
        .get(cid)
        .expect("parked");
    assert!(matches!(car.place, ParkPlace::Kerb { .. }));
    let walked = sim.world().resource::<Agents>().stats.walked;
    // Back home.
    sim.world_mut()
        .resource_mut::<CitizenGrid>()
        .set_location(cid, Location::Work);
    send(&mut sim, cid, goal, home);
    let mut fetching = false;
    for _ in 0..(TICKS_PER_DAY / 2) {
        sim.tick_once();
        let book = sim.world().resource::<TripBook>();
        if book
            .iter()
            .any(|(_, t)| t.park.fetch == cp_citizen::Fetch::ToCar)
        {
            fetching = true;
            // The car stands until its owner is at it.
            assert!(sim.world().resource::<Agents>().parked.get(cid).is_some());
        }
        if book.is_empty() {
            break;
        }
    }
    assert!(fetching, "the trip starts with a walk to the car");
    let agents = sim.world().resource::<Agents>();
    assert!(
        agents.stats.walked >= walked + 1,
        "the walk to the car was walked"
    );
    // The car left the street spot and parks again at home.
    let again = agents.parked.get(cid).expect("parked at home");
    assert_ne!(again.place, car.place);
    assert!(sim.world().resource::<TripBook>().is_empty());
}

//! Format 12: vehicle lane state. The committed v11 fixture (placeholder vehicle motion)
//! migrates and keeps running; v12 saves taken while cars are driving round-trip and resume
//! identically.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_citizen::{
    CitizenGrid, CitizenId, Location, RoutineSettings, Trip, TripBook, TripMode, TripPurpose,
    TripState,
};
use cp_content::{ContentDb, ZoneType};
use cp_core::Arena;
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{AgentStatsV11, AgentsV11, NetGraphV54, SaveDataV11, encode_v11};
use cp_save::*;
use cp_sim::SimRunner;
use cp_traffic::Agents;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/saves")
            .join(name),
    )
    .unwrap()
}

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn runner(seed: u64) -> SimRunner {
    let mut sim = SimRunner::new(seed);
    sim.set_content(content());
    sim
}

#[test]
fn v11_fixture_migrates_and_keeps_running() {
    let bytes = fixture("v11-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 11);
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    let a = &l.data.agents;
    assert!(
        a.pedestrian_count() + a.vehicle_count() > 0,
        "the fixture has an agent"
    );
    assert!(a.load.inst.is_empty() && a.load.ema.is_empty());

    // The frozen v11 writer reproduces the committed file byte for byte (the fixture's agent
    // is a pedestrian, which did not change).
    assert_eq!(a.vehicle_count(), 0);
    let mut pedestrians = Arena::new();
    for (_, p) in a.pedestrians() {
        pedestrians.insert(p.clone());
    }
    let v11 = SaveDataV11 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: NetGraphV54::from(&l.data.net),
        zones: l.data.zones.clone().to_v34(),
        buildings: cp_save::migrate::BuildingGridV12::from(&l.data.buildings),
        citizens: (&l.data.citizens).into(),
        paths: l.data.paths.clone(),
        utilities: l.data.utilities.clone(),
        trips: l.data.trips.clone(),
        agents: AgentsV11 {
            pedestrians,
            vehicles: Arena::new(),
            stats: AgentStatsV11 {
                abstract_trips: a.stats.abstract_trips,
                walked: a.stats.walked,
                driven: a.stats.driven,
                no_path: a.stats.no_path,
                invalid_routes: a.stats.invalid_routes,
                red_waits: a.stats.red_waits,
            },
        },
    };
    let again = encode_v11(&v11, h.content_hash, &h.map_id, h.map_hash).unwrap();
    let l2 = load(&again, h.content_hash).unwrap();
    let peds = |a: &Agents| a.pedestrians().map(|(_, p)| p.clone()).collect::<Vec<_>>();
    assert!(
        peds(&l2.data.agents) == peds(&l.data.agents),
        "frozen v11 writer round-trips"
    );
    assert!(l2.data.agents.stats == l.data.agents.stats);

    // Loaded into a runner it keeps running and re-saves at the current format.
    let mut sim = runner(1);
    l.data.restore(sim.world_mut());
    for _ in 0..TICKS_PER_DAY {
        sim.tick_once();
    }
    let v12 = save(&SaveData::capture(sim.world()), h.content_hash, &h.map_id).unwrap();
    assert_eq!(read_header(&v12).unwrap().0.format_version, FORMAT_VERSION);
    assert!(load(&v12, h.content_hash).unwrap().data.agents == *sim.world().resource::<Agents>());
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

/// Puts `count` adults who are at home on a trip each, to destinations spread over the
/// active buildings. `mode(i)` picks the mode of the i-th trip.
fn inject(sim: &mut SimRunner, count: usize, mode: impl Fn(usize) -> TripMode) -> usize {
    let buildings: Vec<BuildingId> = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .map(|(id, _)| id)
        .collect();
    let movers: Vec<(CitizenId, BuildingId)> = sim
        .world()
        .resource::<CitizenGrid>()
        .citizens()
        .filter(|(_, c)| c.is_adult() && c.location == Location::Home)
        .filter_map(|(id, c)| c.home.map(|h| (id, h)))
        .take(count)
        .collect();
    let mut made = 0;
    sim.world_mut()
        .resource_scope(|w, mut book: bevy_ecs::change_detection::Mut<TripBook>| {
            book.ensure_derived();
            let mut grid = w.resource_mut::<CitizenGrid>();
            for (i, (cid, home)) in movers.into_iter().enumerate() {
                let mut to = buildings[(i * 7 + 3) % buildings.len()];
                if to == home {
                    to = buildings[(i * 7 + 4) % buildings.len()];
                }
                let ok = book
                    .add(Trip {
                        citizen: cid,
                        from: home,
                        to,
                        purpose: TripPurpose::Work,
                        mode: mode(i),
                        state: TripState::Planned,
                    })
                    .is_some();
                if ok {
                    grid.set_location(cid, Location::Moving);
                    made += 1;
                }
            }
        });
    made
}

#[test]
fn cars_on_the_road_survive_save_load_and_resume_identically() {
    let content = content();
    let mut a = quiet_town(&content, 5, 5, "basic_2l", 300);
    assert!(inject(&mut a, 60, |_| TripMode::Car) > 0);
    // Run until some cars are inside junctions.
    let mut on_road = false;
    for _ in 0..200 {
        a.tick_once();
        let ag = a.world().resource::<Agents>();
        if ag.vehicles().any(|(_, v)| v.in_conn)
            && ag.vehicles().any(|(_, v)| v.spawned && !v.in_conn)
        {
            on_road = true;
            break;
        }
    }
    assert!(on_road, "cars are driving, some inside a junction");

    let bytes = save_runner(&a, 7, "m").unwrap();
    let loaded = load(&bytes, 7).unwrap();
    assert_eq!(
        save(&loaded.data, 7, "m").unwrap(),
        bytes,
        "byte-stable re-save"
    );

    let mut b = runner(99);
    load_into_runner(&mut b, &bytes, 7).unwrap();
    // Not saved: the same knob as in `a`, whose routine was switched off by `quiet_town`.
    b.world_mut().resource_mut::<RoutineSettings>().decide = false;
    assert_eq!(b.checksum(), a.checksum(), "vehicle state is checksummed");
    for _ in 0..(TICKS_PER_DAY / 2) {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(b.checksum(), a.checksum());
    assert!(a.world().resource::<Agents>() == b.world().resource::<Agents>());
    assert!(a.world().resource::<Agents>().stats.driven > 0);
}

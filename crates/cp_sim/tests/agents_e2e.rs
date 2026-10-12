//! T-034 acceptance: the agent pools, pedestrians and abstract trips.
//!
//! A town is grown with the real systems, then the routine is switched off
//! (`RoutineSettings::decide = false`) and trips are injected directly, so the numbers in
//! the assertions are exact.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_citizen::{
    CitizenGrid, CitizenId, Location, RoutineSettings, Trip, TripBook, TripMode, TripPurpose,
    TripState,
};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, JunctionGeometry, NetGraph};
use cp_sim::SimRunner;
use cp_traffic::{AgentSettings, Agents, PED_STEP, PathNetwork, crossing_of};
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
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

fn set_pools(sim: &mut SimRunner, pedestrians: u32, vehicles: u32) {
    let mut l = sim.world_mut().resource_mut::<cp_core::SimLimits>();
    l.pedestrians = pedestrians;
    l.vehicles = vehicles;
}

fn counts(sim: &SimRunner) -> (usize, usize, usize) {
    let a = sim.world().resource::<Agents>();
    (
        a.pedestrian_count(),
        a.vehicle_count(),
        sim.world().resource::<TripBook>().len(),
    )
}

#[test]
fn a_pool_of_ten_still_finishes_a_hundred_trips_and_drains() {
    let content = content();
    let mut sim = quiet_town(&content, 5, 5, "basic_2l", 300);
    set_pools(&mut sim, 10, 10);
    let (done0, cancelled0) = {
        let b = sim.world().resource::<TripBook>();
        (b.completed, b.cancelled)
    };
    let made = inject(&mut sim, 100, |i| {
        if i % 5 < 3 {
            TripMode::Walk
        } else {
            TripMode::Car
        }
    });
    assert_eq!(made, 100);
    let mut peak = (0, 0);
    for _ in 0..(3 * TICKS_PER_DAY) {
        sim.tick_once();
        let (p, v, t) = counts(&sim);
        assert!(
            p <= 10 && v <= 10,
            "pool overrun: {p} pedestrians, {v} vehicles"
        );
        peak = (peak.0.max(p), peak.1.max(v));
        if t == 0 {
            break;
        }
    }
    let b = sim.world().resource::<TripBook>();
    let a = sim.world().resource::<Agents>();
    assert!(b.is_empty(), "all trips ended");
    assert_eq!(b.completed - done0, 100, "all 100 completed");
    assert_eq!(b.cancelled - cancelled0, 0);
    assert_eq!(
        (a.pedestrian_count(), a.vehicle_count()),
        (0, 0),
        "no agent leaked"
    );
    assert!(peak.0 == 10 && peak.1 == 10, "pools were used up: {peak:?}");
    assert!(a.stats.abstract_trips > 0, "overflow trips ran abstractly");
    assert_eq!(a.stats.no_path + a.stats.invalid_routes, 0);
    assert!(a.stats.walked > 0 && a.stats.driven > 0);
    // Everybody arrived where the trip said: at work.
    let at_work = sim
        .world()
        .resource::<CitizenGrid>()
        .citizens()
        .filter(|(_, c)| c.location == Location::Work)
        .count();
    assert!(at_work >= 100, "{at_work} citizens at work");
}

#[test]
fn pedestrians_wait_for_the_walk_signal_before_crossing() {
    let content = content();
    // Wide streets: every junction of the lattice gets a traffic light.
    let mut sim = quiet_town(&content, 5, 4, "medium_4l", 150);
    let lights = sim
        .world()
        .resource::<JunctionGeometry>()
        .iter()
        .filter(|(_, j)| j.control.light.is_some())
        .count();
    assert!(lights > 0, "the lattice has traffic lights");
    inject(&mut sim, 150, |_| TripMode::Walk);

    let (mut entries, mut starts_on_red) = (0u32, 0u32);
    let mut was_crossing: BTreeMap<cp_traffic::PedestrianId, Option<(cp_net::NodeId, u16)>> =
        BTreeMap::new();
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
        let now = sim.tick().0 - 1; // the tick whose Agents phase just ran
        let net = sim.world().resource::<NetGraph>();
        let geo = sim.world().resource::<JunctionGeometry>();
        let graph = sim.world().resource::<PathNetwork>().graph().clone();
        let agents = sim.world().resource::<Agents>();
        let mut now_crossing = BTreeMap::new();
        for (id, p) in agents.pedestrians() {
            let c = crossing_of(p, net, &graph, geo);
            if let Some((node, link)) = c {
                now_crossing.insert(id, Some((node, link)));
                let entered = was_crossing.get(&id).copied().flatten() != Some((node, link))
                    || p.s <= PED_STEP + 1e-3;
                if entered && p.s <= PED_STEP + 1e-3 {
                    entries += 1;
                    let j = geo.get(node).unwrap();
                    if let Some(plan) = &j.control.light
                        && j.links.pedestrian[usize::from(link)].kind
                            == cp_net::PedLinkKind::Crossing
                        && !plan.walk(link, cp_traffic::drive::signal_tick(now, 0))
                    {
                        starts_on_red += 1;
                    }
                }
            }
        }
        was_crossing = now_crossing;
        if sim.world().resource::<TripBook>().is_empty() {
            break;
        }
    }
    let a = sim.world().resource::<Agents>();
    assert!(entries > 0, "pedestrians crossed streets");
    assert_eq!(starts_on_red, 0, "nobody started crossing on red");
    assert!(a.stats.red_waits > 0, "some pedestrians really had to wait");
    assert_eq!(a.pedestrian_count(), 0);
}

#[test]
fn parallel_and_sequential_stepping_agree_and_runs_repeat() {
    let content = content();
    let run = |parallel: bool| {
        let mut sim = quiet_town(&content, 7, 7, "basic_2l", 900);
        sim.world_mut().resource_mut::<AgentSettings>().parallel = parallel;
        let made = inject(&mut sim, 700, |_| TripMode::Walk);
        assert!(made > 500, "{made} trips injected");
        let mut peak = 0;
        for _ in 0..(TICKS_PER_DAY / 2) {
            sim.tick_once();
            peak = peak.max(sim.world().resource::<Agents>().pedestrian_count());
        }
        assert!(peak > 300, "more than one chunk walks at once ({peak})");
        (sim.checksum(), sim.world().resource::<Agents>().clone())
    };
    let (a, agents_a) = run(true);
    let (b, agents_b) = run(false);
    let (c, _) = run(true);
    assert_eq!(a, b, "thread layout does not change the result");
    assert_eq!(a, c, "same inputs, same checksum");
    assert_eq!(agents_a, agents_b);
}

#[test]
fn agents_and_trips_stay_consistent_over_two_days_of_the_real_routine() {
    let content = content();
    let mut sim = town(&content, 5, 7, "basic_2l", &MIX);
    while sim.world().resource::<CitizenGrid>().citizen_count() < 800 {
        sim.tick_once();
    }
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
        let book = sim.world().resource::<TripBook>();
        let agents = sim.world().resource::<Agents>();
        let mut carried: BTreeMap<cp_citizen::TripId, u32> = BTreeMap::new();
        for (_, p) in agents.pedestrians() {
            *carried.entry(p.trip).or_default() += 1;
        }
        for (_, v) in agents.vehicles() {
            *carried.entry(v.trip).or_default() += 1;
        }
        // Every agent carries a live trip in state Agent, and every such trip has one agent.
        for (tid, n) in &carried {
            assert_eq!(*n, 1);
            assert_eq!(book.get(*tid).map(|t| t.state), Some(TripState::Agent));
        }
        let agent_trips = book
            .iter()
            .filter(|(_, t)| t.state == TripState::Agent)
            .count();
        assert_eq!(agent_trips, carried.len(), "agent trips and agents match");
    }
    let a = sim.world().resource::<Agents>();
    assert!(a.stats.walked + a.stats.driven > 100);
    assert_eq!(a.stats.no_path, 0, "the lattice is connected");
}

#[test]
fn cars_drive_the_lattice_finish_their_trips_and_light_the_load_overlay() {
    let content = content();
    let mut sim = quiet_town(&content, 5, 5, "basic_2l", 300);
    let before = sim.world().resource::<TripBook>().completed;
    let made = inject(&mut sim, 100, |_| TripMode::Car);
    assert!(made >= 50);
    let mut peak_load = 0.0f32;
    let mut peak_cars = 0;
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
        peak_cars = peak_cars.max(sim.world().resource::<Agents>().vehicle_count());
        let view = sim.view();
        peak_load = peak_load.max(
            view.overlays
                .road_load
                .values()
                .copied()
                .fold(0.0, f32::max),
        );
        if sim.world().resource::<TripBook>().is_empty() {
            break;
        }
    }
    let book = sim.world().resource::<TripBook>();
    let a = sim.world().resource::<Agents>();
    assert!(book.is_empty(), "every car trip ended");
    assert_eq!(book.completed - before, made as u64);
    assert!(
        peak_cars > 5,
        "several cars were on the road at once ({peak_cars})"
    );
    assert!(a.stats.driven > 0);
    assert_eq!(a.stats.invalid_vehicles, 0);
    assert!(peak_load > 0.0, "the traffic overlay sees the cars");
    assert!(
        a.load.ema.iter().any(|&x| x > 0.0),
        "the smoothed load follows"
    );
    assert!(
        sim.view().overlays.road_load.values().all(|&x| x == 0.0),
        "an empty road shows no load"
    );
}

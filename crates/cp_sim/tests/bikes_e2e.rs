//! T-322a acceptance (§23.6 items 3 and 5): cyclists ride the bike lanes of a grown town and
//! arrive; with the pedestrian pool full their rides run abstractly and still end; a town whose
//! streets have bike lanes drives less than the same town without them. T-322c (item 4): the
//! EncourageBiking district rides more.

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
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_traffic::{AgentSettings, Agents};
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// An `n`-line lattice of 90 m streets of `road`, blocks zoned in a dense mix.
fn town(content: &Arc<ContentDb>, seed: u64, lines: usize, road: &str) -> SimRunner {
    const MIX: [ZoneType; 4] = [
        ZoneType::ResHigh,
        ZoneType::ResHigh,
        ZoneType::Industrial,
        ZoneType::ComHigh,
    ];
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
                    MIX[n % MIX.len()],
                );
            }
        }
    }
    sim
}

/// Grows `town` until `n` citizens live there.
fn grown(content: &Arc<ContentDb>, seed: u64, lines: usize, road: &str, n: usize) -> SimRunner {
    let mut sim = town(content, seed, lines, road);
    while sim.world().resource::<CitizenGrid>().citizen_count() < n {
        sim.tick_once();
        assert!(sim.tick().0 < 20 * TICKS_PER_DAY, "never reached {n}");
    }
    sim
}

/// Switches the routine off, waits for the town to go quiet, then puts `count` adults at home
/// on a walk each (to buildings spread over the town); the mode is chosen by time on delivery.
fn send_out(sim: &mut SimRunner, count: usize) -> usize {
    // Two days of the routine first: the newcomers settle in.
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
    assert!(sim.world().resource::<TripBook>().is_empty(), "quiet");
    assert!(sim.world().resource::<AgentSettings>().mode_by_time);
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
                if book
                    .add(Trip {
                        citizen: cid,
                        from: home,
                        to,
                        purpose: TripPurpose::Work,
                        mode: TripMode::Walk,
                        state: TripState::Planned,
                        started: cp_core::Tick(0),
                        park: Default::default(),
                    })
                    .is_some()
                {
                    grid.set_location(cid, Location::Moving);
                    made += 1;
                }
            }
        });
    made
}

/// Runs until every trip ended (at most two days); the most cyclists seen at once.
fn drain(sim: &mut SimRunner) -> usize {
    let mut most = 0;
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
        let riding = sim
            .world()
            .resource::<Agents>()
            .pedestrians()
            .filter(|(_, p)| p.bike)
            .count();
        most = most.max(riding);
        if sim.world().resource::<TripBook>().is_empty() {
            break;
        }
    }
    assert!(sim.world().resource::<TripBook>().is_empty(), "all ended");
    most
}

/// §23.6 item 3: citizens with a bike ride the bike lanes and arrive; their trips complete and
/// none is lost; with a pedestrian pool of five the rest ride abstractly and still arrive.
#[test]
fn cyclists_ride_the_bike_lanes_and_arrive_and_a_full_pool_rides_abstractly() {
    let content = content();
    let mut sim = grown(&content, 5, 4, "basic_2l_bike", 200);
    let done0 = sim.world().resource::<TripBook>().completed;
    let made = send_out(&mut sim, 120);
    assert!(made >= 60, "{made} sent out");
    let done_quiet = sim.world().resource::<TripBook>().completed;
    let a0 = sim.world().resource::<Agents>().stats.clone();
    let most = drain(&mut sim);
    let a = sim.world().resource::<Agents>().stats.clone();
    let book = sim.world().resource::<TripBook>();
    assert_eq!(book.completed - done_quiet, made as u64, "{done0}");
    assert!(most > 0, "cyclists were on the bike lanes");
    assert!(a.ridden > a0.ridden, "rides arrived: {a:?}");
    assert_eq!(a.invalid_routes, a0.invalid_routes);
    assert_eq!(a.no_path, a0.no_path);

    // The same with five pedestrians at most: the rides over the pool run abstractly.
    let mut sim = grown(&content, 5, 4, "basic_2l_bike", 200);
    {
        let mut l = sim.world_mut().resource_mut::<cp_core::SimLimits>();
        l.pedestrians = 5;
    }
    let made = send_out(&mut sim, 120);
    let done_quiet = sim.world().resource::<TripBook>().completed;
    let abstract0 = sim.world().resource::<Agents>().stats.abstract_trips;
    drain(&mut sim);
    let book = sim.world().resource::<TripBook>();
    assert_eq!(book.completed - done_quiet, made as u64);
    assert!(sim.world().resource::<Agents>().stats.abstract_trips > abstract0);
}

/// Car trips and rides under way over `days` days of the real routine (each trip counted
/// once, by the mode it travels in).
fn car_and_bike_trips(sim: &mut SimRunner, days: u64) -> (u64, u64) {
    let mut cars = std::collections::BTreeSet::new();
    let mut rides = std::collections::BTreeSet::new();
    for _ in 0..days * TICKS_PER_DAY {
        sim.tick_once();
        for (id, t) in sim.world().resource::<TripBook>().iter() {
            if matches!(t.state, TripState::Agent | TripState::InTransit { .. }) {
                match t.mode {
                    TripMode::Car => {
                        cars.insert((id, t.started));
                    }
                    TripMode::Bike => {
                        rides.insert((id, t.started));
                    }
                    _ => {}
                }
            }
        }
    }
    (cars.len() as u64, rides.len() as u64)
}

/// §23.6 item 5 (the goal of the task): the same dense town, grown alike, with plain streets
/// and with bike lanes on them: with the bike lanes at least a fifth fewer car trips.
#[test]
fn bike_lanes_take_car_trips_off_the_streets() {
    let content = content();
    let run = |road: &str| {
        let mut sim = grown(&content, 11, 6, road, 1200);
        car_and_bike_trips(&mut sim, 2)
    };
    let (cars_plain, rides_plain) = run("basic_2l");
    let (cars_lanes, rides_lanes) = run("basic_2l_bike");
    eprintln!("plain: {cars_plain} car / {rides_plain} bike; lanes: {cars_lanes} / {rides_lanes}");
    assert!(rides_lanes > rides_plain, "more rides on the bike lanes");
    assert!(
        cars_lanes * 5 <= cars_plain * 4,
        "car trips {cars_lanes} vs {cars_plain}"
    );
}

/// Trips and rides of `days` days of the routine, by the half of the town the citizen lives in
/// (`[west, east]`, split at `split_x`): `(trips, rides)` each.
fn rides_by_half(sim: &mut SimRunner, days: u64, split_x: f32) -> [(u64, u64); 2] {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = [(0, 0); 2];
    for _ in 0..days * TICKS_PER_DAY {
        sim.tick_once();
        let w = sim.world();
        let (book, grid, buildings) = (
            w.resource::<TripBook>(),
            w.resource::<CitizenGrid>(),
            w.resource::<BuildingGrid>(),
        );
        for (id, t) in book.iter() {
            if !matches!(t.state, TripState::Agent | TripState::InTransit { .. })
                || !seen.insert((id, t.started))
            {
                continue;
            }
            let Some(home) = grid.citizen(t.citizen).and_then(|c| buildings.get(c.home?)) else {
                continue;
            };
            let half = &mut out[usize::from(home.pos.x >= split_x)];
            half.0 += 1;
            half.1 += u64::from(t.mode == TripMode::Bike);
        }
    }
    out
}

/// §23.6 item 4 (T-322c): EncourageBiking on the western district of a town with bike lanes:
/// a larger share of rides from its homes than from the same homes without the policy, and
/// than from the eastern half beside it.
#[test]
fn encourage_biking_raises_the_share_of_rides_in_its_district() {
    use cp_district::{DistrictCommand, DistrictId};
    let content = content();
    let split_x = 2000.0 + 225.0;
    let run = |policy: bool| {
        let mut sim = grown(&content, 11, 6, "basic_2l_bike", 1200);
        let cells: Vec<cp_district::Cell> = (50..(split_x / cp_district::CELL_M) as u16)
            .flat_map(|x| (50..90).map(move |z| (x, z)))
            .collect();
        let cmds = [
            DistrictCommand::Create {
                name: "West".into(),
            },
            DistrictCommand::Paint {
                cells,
                district: DistrictId(1),
            },
            DistrictCommand::SetPolicy {
                district: Some(DistrictId(1)),
                policy: cp_content::PolicyKind::EncourageBiking,
                on: policy,
            },
        ];
        for c in cmds {
            sim.push_command(cp_sim::SimCommand::District(c), sim.tick());
            sim.tick_once();
        }
        rides_by_half(&mut sim, 2, split_x)
    };
    let share = |(trips, rides): (u64, u64)| rides as f64 / trips.max(1) as f64;
    let [west_off, _] = run(false);
    let [west_on, east_on] = run(true);
    eprintln!("west off {west_off:?}, west on {west_on:?}, east on {east_on:?}");
    assert!(west_on.0 > 100 && east_on.0 > 100, "enough trips");
    assert!(
        share(west_on) > share(west_off),
        "{west_on:?} vs {west_off:?}"
    );
    assert!(
        share(west_on) > share(east_on),
        "{west_on:?} vs {east_on:?}"
    );
}

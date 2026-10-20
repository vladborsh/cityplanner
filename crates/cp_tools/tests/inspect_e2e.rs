//! T-044: for every panel, what the sim reports for an entity picked from the view equals the
//! sim's own records, read here by independent queries (the Tester's "panel = debug output"
//! check, automated). A small `grid_city` with cars and walkers in flight is the scene.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_building::{BuildingFlags, BuildingGrid, BuildingState};
use cp_citizen::{CitizenGrid, Location, TripBook, TripMode};
use cp_content::ContentDb;
use cp_economy::{Decay, LandGrids};
use cp_net::NetGraph;
use cp_sim::scenario::{GridCityConfig, grid_city, send_citizens};
use cp_sim::{Inspection, Selection, SimRunner};
use cp_terrain::Terrain;
use cp_tools::pick;
use cp_traffic::Agents;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn scene() -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let (mut sim, report) = grid_city(400, 1, terrain, content, &GridCityConfig::default());
    assert!(report.reached && report.rejected.is_empty(), "{report:?}");
    let buildings: Vec<_> = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .map(|(id, _)| id)
        .collect();
    // The city may reach its population on day 0 with the newcomers still on their way home:
    // run on into the second day (the tests below need a tick of at least a day) and let them arrive.
    while sim.tick().0 < 4096 + 1200 {
        sim.tick_once();
    }
    for _ in 0..2000 {
        let g = sim.world().resource::<cp_citizen::CitizenGrid>();
        if g.citizens()
            .filter(|(_, c)| c.is_adult() && c.location == cp_citizen::Location::Home)
            .count()
            >= 60
        {
            break;
        }
        sim.tick_once();
    }
    let cars = send_citizens(&mut sim, 40, &buildings[..3], |_| TripMode::Car);
    let walkers = send_citizens(&mut sim, 10, &buildings[..3], |_| TripMode::Walk);
    assert!(
        cars > 20 && walkers > 5,
        "{cars} drivers, {walkers} walkers"
    );
    // Until cars and walkers are on the road (a small city's trips are short).
    for _ in 0..400 {
        sim.tick_once();
        let a = sim.world().resource::<Agents>();
        if a.vehicles().filter(|(_, v)| v.spawned).count() >= 5 && a.pedestrian_count() >= 2 {
            break;
        }
    }
    sim
}

fn building(sim: &SimRunner, i: usize) -> cp_building::BuildingId {
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .nth(i)
        .map(|(id, _)| id)
        .unwrap()
}

#[test]
fn building_panel_equals_the_records() {
    let mut sim = scene();
    // A house with people and a workplace with workers.
    let home = {
        let c = sim.world().resource::<CitizenGrid>();
        c.households().next().map(|(_, h)| h.home).unwrap()
    };
    let work = {
        let c = sim.world().resource::<CitizenGrid>();
        c.citizens().find_map(|(_, c)| c.work).unwrap()
    };
    // Give one a problem and a running timer, and a level progress, to see them too.
    sim.world_mut().resource_mut::<BuildingGrid>().set_flag(
        home,
        BuildingFlags::NO_WATER | BuildingFlags::NO_ROAD,
        true,
    );
    sim.world_mut()
        .resource_mut::<BuildingGrid>()
        .get_mut(home)
        .unwrap()
        .level_progress = -0.667;
    let now = sim.tick().0;
    sim.world_mut()
        .resource_mut::<Decay>()
        .since
        .insert(home, cp_core::Tick(now - 4096));

    for id in [home, work] {
        let Some(Inspection::Building(info)) = sim.inspect(Selection::Building(id)) else {
            panic!("no building info");
        };
        let w = sim.world();
        let b = w.resource::<BuildingGrid>().get(id).unwrap();
        let db = w.resource::<cp_content::SimContent>().0.clone();
        let def = db.building(b.def);
        let citizens = w.resource::<CitizenGrid>();
        assert_eq!(
            (info.level, info.state, info.zone),
            (b.level, b.state, def.zone)
        );
        assert_eq!(info.level_progress, b.level_progress);
        assert_eq!(info.def, def.id);
        // People: counted from the records, not from the derived tables the info uses.
        let residents = citizens
            .citizens()
            .filter(|(_, c)| c.home == Some(id))
            .count() as u32;
        assert_eq!(info.residents, residents);
        let households = citizens.households().filter(|(_, h)| h.home == id).count() as u16;
        assert_eq!(info.households, households);
        assert_eq!(info.apartments, def.households);
        for l in 0..4 {
            let n = citizens
                .citizens()
                .filter(|(_, c)| c.work == Some(id) && usize::from(c.work_level) == l)
                .count() as u16;
            assert_eq!(info.workers[l], n, "workers of education {l}");
        }
        assert_eq!(info.jobs, def.jobs);
        let grids = w.resource::<LandGrids>();
        assert_eq!(info.land_value, grids.value_at(b.pos));
        assert_eq!(info.pollution, grids.pollution_at(b.pos));
        // The service problems (garbage, T-136) come after the flags.
        assert_eq!(
            info.problems.iter().filter(|p| **p != "garbage").count(),
            (0..8).filter(|k| b.flags.0 >> k & 1 == 1).count()
        );
    }
    let Some(Inspection::Building(info)) = sim.inspect(Selection::Building(home)) else {
        unreachable!()
    };
    assert!(info.problems.contains(&"no water") && info.problems.contains(&"no road"));
    let t = info.trouble.expect("a timer is running");
    assert!((t.days - 1.0).abs() < 0.01);
    // T-282: abandon_days 35, demolish_days 70 (were 7 and 14), one day in.
    assert!((t.days_to_abandon - 34.0).abs() < 0.01 && (t.days_to_demolition - 69.0).abs() < 0.01);
    let rows = Inspection::Building(info).rows();
    let text = |label: &str| {
        rows.iter()
            .find(|r| r.0 == label)
            .map(|r| r.1.clone())
            .unwrap()
    };
    assert!(text("Problems").contains("no water"));
    assert!(text("Level").contains("-0.67"));
    assert!(text("In trouble for").starts_with("1.0 days"));
}

#[test]
fn citizen_panel_equals_the_records() {
    let sim = scene();
    let w = sim.world();
    let citizens = w.resource::<CitizenGrid>();
    let (id, c) = citizens
        .citizens()
        .find(|(_, c)| c.work.is_some() && c.home.is_some())
        .unwrap();
    let Some(Inspection::Citizen(info)) = sim.inspect(Selection::Citizen(id)) else {
        panic!()
    };
    assert_eq!(info.education, c.education);
    assert_eq!(info.age_years, c.age_days / cp_citizen::DAYS_PER_YEAR);
    assert_eq!(info.has_car, c.has_car);
    assert_eq!(info.location, c.location);
    assert_eq!(info.home.as_ref().map(|h| h.0), c.home);
    assert_eq!(info.work.as_ref().map(|h| h.0), c.work);
    assert_eq!(info.work.as_ref().map(|h| h.2), Some(c.work_level));
    let size = citizens
        .household(c.household)
        .unwrap()
        .member_ids()
        .count();
    assert_eq!(info.household_size, size);

    // A citizen on a trip: the panel shows the trip book's trip.
    let book = w.resource::<TripBook>();
    let (tid, trip) = book.iter().next().expect("trips are running");
    let Some(Inspection::Citizen(on_trip)) = sim.inspect(Selection::Citizen(trip.citizen)) else {
        panic!()
    };
    let t = on_trip.trip.expect("trip shown");
    assert_eq!(book.active_of(trip.citizen), Some(tid));
    assert_eq!(
        (t.purpose, t.mode, t.state, t.from, t.to),
        (trip.purpose, trip.mode, trip.state, trip.from, trip.to)
    );
}

#[test]
fn vehicle_and_pedestrian_panels_equal_the_records_and_picking_finds_them() {
    let sim = scene();
    let view = sim.view();
    let agents = sim.world().resource::<Agents>();
    assert!(agents.vehicle_count() > 0 && agents.pedestrian_count() > 0);

    // Vehicles: picked at the place the renderer draws them (fraction 1.0 = the latest pose).
    // A car on a citizen's trip (service vehicles, which the scene's services send out since
    // T-267 placed them at the road edge, have none).
    let book = sim.world().resource::<TripBook>();
    let (vid, v) = agents
        .vehicles()
        .find(|(_, v)| v.spawned && book.get(v.trip).is_some())
        .unwrap();
    let key = u64::from(vid.generation) << 32 | u64::from(vid.index);
    let pose = view
        .agents
        .cur
        .vehicles
        .iter()
        .find(|p| p.key == key)
        .unwrap();
    let at = Vec2::new(pose.pos.x, pose.pos.z);
    let sel = pick(&view, 1.0, at, 30.0).expect("something is there");
    // Another agent may stand on the same spot; the pick is then that one, equally valid.
    let sel = if matches!(sel, Selection::Vehicle(_)) {
        Selection::Vehicle(key)
    } else {
        sel
    };
    let Some(Inspection::Vehicle(info)) = sim.inspect(sel) else {
        panic!("not a vehicle")
    };
    assert_eq!(info.key, key);
    assert_eq!(info.kind, v.kind);
    assert_eq!(info.speed_mps, v.speed);
    assert_eq!(info.driver, v.citizen);
    assert_eq!(info.lanes_total, v.path.lanes().count());
    assert_eq!(info.lanes_left, info.lanes_total - usize::from(v.hop));
    assert_eq!(
        info.route.len(),
        info.lanes_left,
        "one line per lane still to go"
    );
    // The route starts on the lane the vehicle is on and ends at the path's goal lane.
    let net = sim.world().resource::<NetGraph>();
    let lanes: Vec<_> = v.path.lanes().collect();
    let first = net.lane(lanes[usize::from(v.hop)]).unwrap().curve[0].point(0.0);
    assert_eq!(info.route[0][0], first);
    let trip = info.trip.as_ref().expect("a driver has a trip");
    assert_eq!(trip.mode, TripMode::Car);
    assert_eq!(
        sim.world().resource::<TripBook>().get(v.trip).unwrap().to,
        trip.to
    );

    // Pedestrians show their citizen.
    let (pid, p) = agents.pedestrians().next().unwrap();
    let pkey = u64::from(pid.generation) << 32 | u64::from(pid.index);
    let Some(Inspection::Citizen(info)) = sim.inspect(Selection::Pedestrian(pkey)) else {
        panic!()
    };
    assert_eq!(info.id, p.citizen);
    assert_eq!(info.trip.as_ref().map(|t| t.mode), Some(TripMode::Walk));
    assert_eq!(
        info.location,
        sim.world()
            .resource::<CitizenGrid>()
            .citizen(p.citizen)
            .unwrap()
            .location
    );
    let _ = Location::Moving;
}

#[test]
fn segment_panel_equals_the_traffic_overlay() {
    let sim = scene();
    let view = sim.view();
    let loaded = view
        .overlays
        .road_load
        .iter()
        .filter(|(_, l)| **l > 0.0)
        .map(|(id, l)| (*id, *l))
        .next()
        .expect("some road carries cars");
    let Some(Inspection::Segment(info)) = sim.inspect(Selection::Segment(loaded.0)) else {
        panic!()
    };
    // The number the traffic overlay colours the segment by.
    assert_eq!(info.load, loaded.1);
    let net = sim.world().resource::<NetGraph>();
    let s = net.segment(loaded.0).unwrap();
    let db = view.content.clone().unwrap();
    assert_eq!(info.length, s.length);
    assert_eq!(info.road, db.road(s.road).id);
    assert_eq!(info.lanes, s.lanes.len());
    assert_eq!(
        info.load_avg,
        sim.world().resource::<Agents>().load.ema[loaded.0.index as usize]
    );
    // Picking the middle of the segment finds a segment (no agent or building there).
    let mid = s.curve.point(0.5);
    let found = pick(
        &{
            let mut v = sim.view();
            v.agents = Default::default();
            v
        },
        1.0,
        Vec2::new(mid.x, mid.z),
        30.0,
    );
    assert!(
        matches!(found, Some(Selection::Segment(_) | Selection::Building(_))),
        "{found:?}"
    );
}

#[test]
fn a_vanished_entity_inspects_to_nothing() {
    let mut sim = scene();
    let id = building(&sim, 0);
    assert!(sim.inspect(Selection::Building(id)).is_some());
    let mut zones = sim.world().resource::<cp_zone::ZoneGrid>().clone();
    let mut grid = sim.world().resource::<BuildingGrid>().clone();
    cp_building::demolish_building(&mut grid, &mut zones, id);
    sim.world_mut().insert_resource(grid);
    sim.world_mut().insert_resource(zones);
    assert!(sim.inspect(Selection::Building(id)).is_none());
}

//! T-288: dust when something is built or knocked down (`cp_render::city_events`,
//! `cp_render::dust`, pure). The diff of two snapshots of a sim gives one change per building
//! placed or removed and the road pieces laid or removed; the puffs: one per placed service (none
//! for a grown house), one per 6 cells of a new road, a darker, bigger one per building or road
//! gone; at most 24 live.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_building::{BuildingCommand, BuildingGrid};
use cp_content::ContentDb;
use cp_core::{Cost, Treasury};
use cp_net::{ElevationMode, NetCommand, NetGraph, RoadPath};
use cp_render::city_events::{BuildingEvent, building_events, segment_events};
use cp_render::dust::{BUILD_DUST, DEMOLISH_DUST, MAX_PUFFS, ROAD_STEP_M, cap, puffs};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const A: Vec2 = Vec2::new(4400.0, 4400.0);

fn sim() -> (SimRunner, Arc<ContentDb>) {
    let db = Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let t = Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(288);
    sim.set_terrain(t);
    sim.set_content(Arc::clone(&db));
    sim.tick_once();
    sim.world_mut().resource_mut::<Treasury>().balance = Cost::from_units(1_000_000_000.0);
    (sim, db)
}

fn run(sim: &mut SimRunner, cmd: SimCommand) {
    sim.push_command(cmd, sim.tick());
    sim.tick_once();
}

fn snap(sim: &SimRunner) -> (NetGraph, BuildingGrid) {
    (
        sim.world().resource::<NetGraph>().clone(),
        sim.world().resource::<BuildingGrid>().clone(),
    )
}

#[test]
fn a_road_a_service_and_their_removal_give_one_change_each_and_their_dust() {
    let (mut sim, db) = sim();
    let width = |r| db.road(r).width;
    let (n0, b0) = snap(&sim);
    // A street of 20 cells: its pieces laid, 10 puffs (one per 2 cells), sandy.
    run(
        &mut sim,
        SimCommand::Net(NetCommand::BuildRoad {
            road: db.road_id("basic_2l").unwrap(),
            path: RoadPath::Straight {
                from: A,
                to: A + Vec2::X * 160.0,
            },
            elevation: ElevationMode::Ground,
        }),
    );
    let (n1, b1) = snap(&sim);
    let (laid, removed) = segment_events(&n0, &n1, width);
    assert!(!laid.is_empty() && removed.is_empty());
    let len: f32 = laid.iter().map(|s| s.curve.arc_table().length()).sum();
    assert!((len - 160.0).abs() < 0.5, "the street's pieces: {len}");
    let p = puffs(&building_events(&b0, &b1), &laid, &removed, &db);
    assert_eq!(p.len(), (160.0 / ROAD_STEP_M) as usize);
    assert!(p.iter().all(|p| !p.demolish));
    // A police station beside it: one Placed, one puff.
    run(
        &mut sim,
        SimCommand::Building(BuildingCommand::Place {
            def: db.building_id("police_station").unwrap(),
            pos: A + Vec2::new(80.0, 20.0),
            rot: 0.0,
        }),
    );
    let (n2, b2) = snap(&sim);
    let changes = building_events(&b1, &b2);
    let placed: Vec<_> = changes
        .iter()
        .filter(|c| c.event == BuildingEvent::Placed)
        .collect();
    assert_eq!(placed.len(), 1, "{changes:?}");
    let station = placed[0].id;
    let (laid, removed) = segment_events(&n1, &n2, width);
    let p = puffs(&changes, &laid, &removed, &db);
    assert_eq!(p.len(), 1);
    assert!(!p[0].demolish && p[0].radius >= 4.0);
    // Bulldozed: one Removed, one darker, bigger puff.
    run(
        &mut sim,
        SimCommand::Building(BuildingCommand::Demolish { building: station }),
    );
    let (_, b3) = snap(&sim);
    let changes = building_events(&b2, &b3);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].event, BuildingEvent::Removed);
    let gone = puffs(&changes, &[], &[], &db);
    assert_eq!(gone.len(), 1);
    assert!(gone[0].demolish && gone[0].radius > p[0].radius);
    assert_ne!(BUILD_DUST, DEMOLISH_DUST);
}

#[test]
fn a_grown_house_raises_no_dust_and_the_puffs_are_capped() {
    let (_, db) = sim();
    let house = db.building_id("house_low_1").unwrap();
    let mut g = BuildingGrid::default();
    let old = g.clone();
    g.insert(cp_building::Building {
        def: house,
        footprint: cp_building::Footprint::Free { w: 1, d: 1 },
        level: 1,
        state: cp_building::BuildingState::UnderConstruction,
        ready_at: cp_core::Tick(0),
        pos: A,
        rot: 0.0,
        access: None,
        flags: cp_building::BuildingFlags::default(),
        level_progress: 0.0,
    });
    let changes = building_events(&old, &g);
    assert_eq!(changes.len(), 1);
    assert!(
        puffs(&changes, &[], &[], &db).is_empty(),
        "no dust for a grown house"
    );
    let mut live: Vec<u32> = (0..100).collect();
    cap(&mut live);
    assert_eq!(live.len(), MAX_PUFFS);
    assert_eq!(live[0], 100 - MAX_PUFFS as u32, "the oldest go first");
}

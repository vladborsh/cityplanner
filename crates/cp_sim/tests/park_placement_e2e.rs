//! T-223: the park buildings stand only in a park area of their kind (a gate touches it, an
//! attraction is inside it at its level, a prop is in any park area), and `ParkState` adds up
//! the active buildings of each area.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::BuildingCommand;
use cp_content::{ContentDb, ParkKind};
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome};
use cp_district::{AreaId, AreaKind, DistrictCommand};
use cp_net::{Elevation, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{GRID, MapMeta, RES_GRID, ResourceMap, Terrain, TileOwnership};
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// Flat dry ground; forest and ore are rich in the columns left of x = 3136 m (pixels < 49),
/// nothing to the right.
fn terrain() -> Terrain {
    let meta = MapMeta {
        name: "flat".into(),
        sea_level: 0.0,
        start_tile: (1, 1),
        external_connections: vec![],
        entry_road: None,
        entry_rail: None,
    };
    let px = (0..RES_GRID * RES_GRID)
        .map(|i| {
            if i % RES_GRID < 49 {
                [0, 255, 255, 0]
            } else {
                [0; 4]
            }
        })
        .collect();
    Terrain::from_samples(meta, &vec![1000; GRID * GRID], &vec![0; GRID * GRID])
        .unwrap()
        .with_resources(ResourceMap::from_pixels(px).unwrap())
}

/// One 200 m road at z = 3000 on an owned tile, with the flat terrain.
fn world(c: &Arc<ContentDb>) -> SimRunner {
    let road = c.road_id("basic_2l").unwrap();
    let def = c.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(c));
    sim.set_terrain(Arc::new(terrain()));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3200.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = cp_core::Cost::from_units(5.0e6);
    sim.tick_once();
    sim
}

fn run(sim: &mut SimRunner, cmd: SimCommand) -> Result<(), String> {
    sim.push_command(cmd, sim.tick());
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    match out.last().unwrap().result.clone() {
        Ok(_) => Ok(()),
        Err(CommandError::Rejected { reason, .. }) => Err(reason),
        Err(e) => panic!("{e:?}"),
    }
}

fn place(sim: &mut SimRunner, c: &ContentDb, id: &str, pos: Vec2) -> Result<(), String> {
    let def = c.building_id(id).unwrap();
    run(
        sim,
        SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
    )
}

fn spot(sim: &SimRunner, col: u16) -> Vec2 {
    let zones = sim.world().resource::<ZoneGrid>();
    let (id, _) = zones.blocks().next().unwrap();
    zones
        .cell_footprint(ZoneCellRef {
            block: id,
            cell: col * 4 + 1,
        })
        .unwrap()
        .c
}

/// Creates an area of `kind` and paints the cells around the road (x 2880..3200, z 2816..3200).
fn area(sim: &mut SimRunner, kind: AreaKind) -> AreaId {
    run(
        sim,
        SimCommand::District(DistrictCommand::CreateArea {
            kind,
            name: "Area".into(),
        }),
    )
    .unwrap();
    let id = AreaId(sim.view().areas.iter().last().unwrap().0.0);
    let cells = (90..100)
        .flat_map(|c| (88..100).map(move |r| (c, r)))
        .collect();
    run(
        sim,
        SimCommand::District(DistrictCommand::PaintArea { cells, area: id }),
    )
    .unwrap();
    id
}

fn park_area(sim: &mut SimRunner, kind: ParkKind) -> AreaId {
    area(sim, AreaKind::Park(kind))
}

#[test]
fn a_gate_touches_an_area_of_its_kind_and_the_rest_wait_for_the_area_and_its_level() {
    let c = content();
    let mut sim = world(&c);
    let gate_at = spot(&sim, 5);
    // No area at all.
    let e = place(&mut sim, &c, "zoo_gate", gate_at).unwrap_err();
    assert!(e.contains("Zoo park area"), "{e}");
    let (attr_at, outside) = (Vec2::new(3100.0, 3100.0), Vec2::new(3100.0, 2780.0));
    let e = place(&mut sim, &c, "zoo_attr_1", attr_at).unwrap_err();
    assert!(e.contains("inside a Zoo park area"), "{e}");
    let e = place(&mut sim, &c, "prop_bench", attr_at).unwrap_err();
    assert!(e.contains("inside a park area"), "{e}");

    // A Nature area: the zoo gate and attraction are refused, the nature gate stands.
    let nature = park_area(&mut sim, ParkKind::Nature);
    let e = place(&mut sim, &c, "zoo_gate", gate_at).unwrap_err();
    assert!(e.contains("Zoo park area"), "{e}");
    let e = place(&mut sim, &c, "zoo_attr_1", attr_at).unwrap_err();
    assert!(e.contains("inside a Zoo park area"), "{e}");
    place(&mut sim, &c, "nature_gate", gate_at).unwrap();
    // A prop stands in any park area, not outside it.
    place(&mut sim, &c, "prop_bench", Vec2::new(3180.0, 3100.0)).unwrap();
    let e = place(&mut sim, &c, "prop_bench", outside).unwrap_err();
    assert!(e.contains("inside a park area"), "{e}");

    // Repaint the ground as a Zoo area: now the zoo gate and attraction stand; level 1 holds
    // attraction 1 only.
    run(
        &mut sim,
        SimCommand::District(DistrictCommand::DeleteArea { area: nature }),
    )
    .unwrap();
    let zoo = park_area(&mut sim, ParkKind::Zoo);
    place(&mut sim, &c, "zoo_attr_1", attr_at).unwrap();
    let e = place(&mut sim, &c, "zoo_attr_2", Vec2::new(3140.0, 3140.0)).unwrap_err();
    assert!(e.contains("needs area level 2 (it is 1)"), "{e}");
    sim.world_mut()
        .resource_mut::<cp_district::Areas>()
        .get_mut(zoo)
        .unwrap()
        .level = 2;
    place(&mut sim, &c, "zoo_attr_2", Vec2::new(3140.0, 3140.0)).unwrap();
    // The attraction needs no road: outside the area it is refused.
    let e = place(&mut sim, &c, "zoo_attr_1", outside).unwrap_err();
    assert!(e.contains("inside a Zoo park area"), "{e}");
}

#[test]
fn a_gate_far_from_its_area_is_refused_and_a_city_park_needs_no_area() {
    let c = content();
    let mut sim = world(&c);
    // The area is a small patch: the cell of the first spot only.
    let at = spot(&sim, 20);
    let cell = cp_district::DistrictMap::cell_of(at).unwrap();
    run(
        &mut sim,
        SimCommand::District(DistrictCommand::CreateArea {
            kind: AreaKind::Park(ParkKind::Zoo),
            name: "Zoo".into(),
        }),
    )
    .unwrap();
    run(
        &mut sim,
        SimCommand::District(DistrictCommand::PaintArea {
            cells: vec![cell],
            area: AreaId(1),
        }),
    )
    .unwrap();
    place(&mut sim, &c, "zoo_gate", at).unwrap();
    let far = spot(&sim, 2);
    assert!(far.distance(at) > 100.0, "{far} {at}");
    let e = place(&mut sim, &c, "zoo_gate", far).unwrap_err();
    assert!(e.contains("touch a Zoo park area"), "{e}");
    // The four basic city parks stand on a road with no area (T-221).
    place(&mut sim, &c, "small_park", far).unwrap();
}

/// A building put straight into the grid, already active.
fn put(sim: &mut SimRunner, c: &ContentDb, id: &str, pos: Vec2) {
    use cp_building::{Building, BuildingFlags, BuildingState, Footprint};
    let def = c.building_id(id).unwrap();
    let d = c.building(def);
    sim.world_mut()
        .resource_mut::<cp_building::BuildingGrid>()
        .insert(Building {
            def,
            footprint: Footprint::Free {
                w: d.lot.0,
                d: d.lot.1,
            },
            level: 1,
            state: BuildingState::Active,
            ready_at: cp_core::Tick(0),
            pos,
            rot: 0.0,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
}

#[test]
fn the_park_state_adds_up_the_attraction_and_capacity_of_the_buildings_of_the_area() {
    let c = content();
    let mut sim = world(&c);
    let zoo = park_area(&mut sim, ParkKind::Zoo);
    let attract = |sim: &SimRunner| sim.view().parks.get(zoo).copied().unwrap();
    sim.tick_once();
    assert_eq!((attract(&sim).attract, attract(&sim).capacity), (0.0, 0));
    put(&mut sim, &c, "zoo_attr_1", Vec2::new(3100.0, 3100.0));
    put(&mut sim, &c, "zoo_attr_2", Vec2::new(3140.0, 3140.0));
    put(&mut sim, &c, "prop_bench", Vec2::new(3180.0, 3100.0));
    // The attraction of another kind in this area counts for nothing; one outside it too.
    put(&mut sim, &c, "nature_attr_1", Vec2::new(3060.0, 3100.0));
    put(&mut sim, &c, "zoo_attr_1", Vec2::new(3100.0, 2780.0));
    sim.tick_once();
    let s = attract(&sim);
    // (10 + 20 + 1) x (1 + 0.1 x level 1); 40 + 80 visitors at once.
    assert!((s.attract - 31.0 * 1.1).abs() < 1e-3, "{}", s.attract);
    assert_eq!(s.capacity, 120);
    // The level raises the attractiveness at the next hourly count.
    sim.world_mut()
        .resource_mut::<cp_district::Areas>()
        .get_mut(zoo)
        .unwrap()
        .level = 3;
    for _ in 0..cp_park::PARK_RECOUNT_TICKS {
        sim.tick_once();
    }
    assert!((attract(&sim).attract - 31.0 * 1.3).abs() < 1e-3);
    // Deleting the area drops its state.
    run(
        &mut sim,
        SimCommand::District(DistrictCommand::DeleteArea { area: zoo }),
    )
    .unwrap();
    sim.tick_once();
    sim.tick_once();
    for _ in 0..cp_park::PARK_RECOUNT_TICKS {
        sim.tick_once();
    }
    assert!(sim.view().parks.is_empty());
}

#[test]
fn the_park_content_is_complete_and_the_state_replays_the_same() {
    let c = content();
    let n = c.buildings().filter(|(_, d)| d.park.is_some()).count();
    assert_eq!(n, 30, "26 park-area buildings and the 4 city parks");
    for k in ["city_park", "nature", "zoo", "amusement"] {
        assert!(c.building_id(&format!("{k}_gate")).is_some(), "{k}");
        for i in 1..=4 {
            let id = c.building_id(&format!("{k}_attr_{i}")).unwrap();
            assert_eq!(c.building(id).park.as_ref().unwrap().min_level, i);
        }
    }
    let run_once = || {
        let mut sim = world(&c);
        park_area(&mut sim, ParkKind::Zoo);
        put(&mut sim, &c, "zoo_attr_1", Vec2::new(3100.0, 3100.0));
        for _ in 0..400 {
            sim.tick_once();
        }
        sim.checksum()
    };
    assert_eq!(run_once(), run_once());
}

//! T-030 acceptance: service buildings are placed beside roads and demolished through the
//! command queue; invalid placements are rejected and change nothing.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingGrid, BuildingState};
use cp_content::{BuildingDefId, ContentDb};
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandLog, CommandOutcome};
use cp_net::{Elevation, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::TileOwnership;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// One 200 m road at z = 3000 on an owned tile; blocks built.
fn world(content: &Arc<ContentDb>) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(content));
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
    sim.tick_once();
    sim
}

fn run(sim: &mut SimRunner, cmd: BuildingCommand) -> Result<(), String> {
    sim.push_command(SimCommand::Building(cmd), sim.tick());
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

/// A service building that stands on a road: the coal plant (3×3; the turbine is free-standing now).
fn pump(c: &ContentDb) -> BuildingDefId {
    c.building_id("coal_power_plant").unwrap()
}

/// Centre of a valid cell in row 1 of the first block, column `col`.
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

#[test]
fn place_then_demolish_round_trip() {
    let c = content();
    let mut sim = world(&c);
    let rev = sim.world().resource::<BuildingGrid>().revision();
    let zrev = sim.world().resource::<ZoneGrid>().revision();
    let at = spot(&sim, 5);
    run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: at,
            rot: 0.0,
        },
    )
    .unwrap();
    let (id, cells) = {
        let g = sim.world().resource::<BuildingGrid>();
        assert_eq!(g.len(), 1);
        assert_ne!(g.revision(), rev);
        let (id, b) = g.iter().next().unwrap();
        // T-175: a service stands ready at once.
        assert_eq!(b.state, BuildingState::Active);
        // T-267: a service stands at the road edge, a rectangle with an access point.
        assert!(b.lot().is_none() && b.access().is_some());
        assert_eq!(b.size(), (3, 3));
        (id, b.footprint_polygon())
    };
    // The next tick lays the blockers: the cells under the rectangle are invalid.
    sim.tick_once();
    let under = |sim: &SimRunner| {
        let z = sim.world().resource::<ZoneGrid>();
        z.cells_within(cells.iter().sum::<Vec2>() / 4.0, 20.0)
            .into_iter()
            .filter(|&c| {
                z.cell_footprint(c)
                    .is_some_and(|o| cp_zone::geom::quads_overlap(&cells, &o.corners()))
            })
            .collect::<Vec<_>>()
    };
    {
        let z = sim.world().resource::<ZoneGrid>();
        assert_ne!(z.revision(), zrev);
        let cells_under = under(&sim);
        assert!(!cells_under.is_empty());
        assert!(
            cells_under.iter().any(|&x| !z.is_valid(x)),
            "the cells it covers go invalid"
        );
    }
    assert_eq!(sim.world().resource::<CommandLog<SimCommand>>().len(), 1);

    // A second building at the same cursor stands beside the first, not on it.
    run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: at,
            rot: 0.0,
        },
    )
    .unwrap();
    {
        let g = sim.world().resource::<BuildingGrid>();
        assert_eq!(g.len(), 2);
        let quads: Vec<_> = g.iter().map(|(_, b)| b.footprint_polygon()).collect();
        assert!(!cp_zone::geom::quads_overlap(&quads[0], &quads[1]));
    }
    let second = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .map(|(i, _)| i)
        .find(|&i| i != id)
        .unwrap();
    run(&mut sim, BuildingCommand::Demolish { building: second }).unwrap();

    sim.tick_once();
    let g = sim.world().resource::<BuildingGrid>();
    assert_eq!(g.get(id).unwrap().state, BuildingState::Active);

    let rev = g.revision();
    let zrev = sim.world().resource::<ZoneGrid>().revision();
    run(&mut sim, BuildingCommand::Demolish { building: id }).unwrap();
    let g = sim.world().resource::<BuildingGrid>();
    assert!(g.is_empty() && g.revision() != rev);
    sim.tick_once();
    let z = sim.world().resource::<ZoneGrid>();
    assert_ne!(z.revision(), zrev);
    assert!(under(&sim).iter().all(|&x| z.is_valid(x)));
    // Gone: demolishing again fails; the freed spot takes a building again.
    assert!(run(&mut sim, BuildingCommand::Demolish { building: id }).is_err());
    run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: at,
            rot: 0.0,
        },
    )
    .unwrap();
}

#[test]
fn invalid_placements_are_rejected() {
    let c = content();
    let mut sim = world(&c);
    let place = |pos, def| BuildingCommand::Place { def, pos, rot: 0.0 };
    let s5 = spot(&sim, 5);
    // No road: far from any block.
    let e = run(&mut sim, place(Vec2::new(3100.0, 3300.0), pump(&c))).unwrap_err();
    assert!(e.contains("next to a road"), "{e}");
    // At the last column the lot slides back along the street to fit the block (BUG-082).
    let last = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .next()
        .unwrap()
        .1
        .cols
        - 1;
    let end = spot(&sim, last);
    let plan = {
        let w = sim.world();
        cp_building::validate_placement(
            &c,
            w.resource::<ZoneGrid>(),
            w.resource::<cp_net::NetGraph>(),
            None,
            None,
            pump(&c),
            end,
            0.0,
        )
        .unwrap()
    };
    let cols = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .next()
        .unwrap()
        .1
        .cols;
    assert!(
        plan.cells()
            .iter()
            .all(|c| c.cell / cp_zone::BLOCK_DEPTH < cols)
    );
    // Zoned buildings are not placeable by hand.
    let house = c.building_id("house_low_1").unwrap();
    let e = run(&mut sim, place(s5, house)).unwrap_err();
    assert!(e.contains("by hand"), "{e}");
    // Tile not owned.
    sim.world_mut().insert_resource(TileOwnership::none());
    let e = run(&mut sim, place(s5, pump(&c))).unwrap_err();
    assert!(e.contains("not owned"), "{e}");
    assert!(sim.world().resource::<BuildingGrid>().is_empty());
    assert!(sim.world().resource::<CommandLog<SimCommand>>().is_empty());
}

#[test]
fn overlapping_buildings_are_rejected_but_neighbours_fit() {
    let c = content();
    let mut sim = world(&c);
    let a = spot(&sim, 6);
    let (s7, s9) = (spot(&sim, 7), spot(&sim, 9));
    run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: a,
            rot: 0.0,
        },
    )
    .unwrap();
    // Over the first one: it slides along the road to the free place beside it.
    run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: s7,
            rot: 0.0,
        },
    )
    .unwrap();
    run(
        &mut sim,
        BuildingCommand::Place {
            def: pump(&c),
            pos: s9,
            rot: 0.0,
        },
    )
    .unwrap();
    let g = sim.world().resource::<BuildingGrid>();
    assert_eq!(g.len(), 3);
    let quads: Vec<_> = g.iter().map(|(_, b)| b.footprint_polygon()).collect();
    for i in 0..quads.len() {
        for j in i + 1..quads.len() {
            assert!(!cp_zone::geom::quads_overlap(&quads[i], &quads[j]));
        }
    }
}

/// Flat dry land at 10 m with a lake (ground at 0 m, sea level 5 m) around x 3080..3140.
fn lake_terrain() -> cp_terrain::Terrain {
    use cp_terrain::{GRID, MapMeta, SPACING, Terrain};
    let mut height = vec![655u16; GRID * GRID]; // about 10 m
    for iz in 280..320 {
        for ix in 308..314 {
            height[iz * GRID + ix] = 0;
        }
    }
    let meta = MapMeta {
        name: "lake".into(),
        sea_level: 5.0,
        start_tile: (1, 1),
        external_connections: vec![],
        entry_road: None,
        entry_rail: None,
    };
    let _ = SPACING;
    Terrain::from_samples(meta, &height, &vec![0u16; GRID * GRID]).unwrap()
}

#[test]
fn a_pump_needs_water_and_other_buildings_cannot_stand_in_it() {
    let c = content();
    let mut sim = world(&c);
    sim.world_mut()
        .insert_resource(cp_terrain::SimTerrain(Arc::new(lake_terrain())));
    let cols = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .next()
        .unwrap()
        .1
        .cols;
    let (dry, wet) = (spot(&sim, 3), spot(&sim, 13));
    assert!(cols > 20);
    let pump = c.building_id("water_pump_station").unwrap();
    let turbine = c.building_id("coal_power_plant").unwrap();
    let place = |pos, def| BuildingCommand::Place { def, pos, rot: 0.0 };
    let e = run(&mut sim, place(dry, pump)).unwrap_err();
    assert!(e.contains("on or beside water"), "{e}");
    // A plant asked for in the water stands on the dry land beside the road (T-267).
    run(&mut sim, place(wet, turbine)).unwrap();
    run(&mut sim, place(wet, pump)).unwrap();
    run(&mut sim, place(dry, turbine)).unwrap();
}

#[test]
fn non_finite_positions_and_rotations_are_rejected() {
    let c = content();
    let mut sim = world(&c);
    let at = spot(&sim, 5);
    let def = c.building_id("coal_power_plant").unwrap();
    for (pos, rot) in [
        (at, f32::NAN),
        (at, f32::INFINITY),
        (Vec2::new(f32::NAN, at.y), 0.0),
        (Vec2::new(at.x, f32::INFINITY), 0.0),
        (Vec2::new(1e30, at.y), 0.0),
    ] {
        let e = run(&mut sim, BuildingCommand::Place { def, pos, rot }).unwrap_err();
        assert!(e.contains("invalid"), "{e}");
    }
    assert!(sim.world().resource::<BuildingGrid>().is_empty());
}

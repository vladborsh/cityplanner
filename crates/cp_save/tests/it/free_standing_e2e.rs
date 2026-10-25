//! T-107 acceptance (with the real `water_tower`, free-standing since T-109): a free-standing
//! building is placed in a field far from any road, gets power from a line 40 m away, makes the zone cells under it invalid, frees them again when demolished, and
//! survives save → load with the same invalid cells and the same checksum.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingCommand, BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_content::{BuildingDefId, ContentDb, UtilityKind};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{CommandError, CommandOutcome, SimLimits};
use cp_net::{Elevation, NetGraph};
use cp_save::{load_into_runner, save_runner};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{Terrain, TileOwnership};
use cp_utility::UtilityCommand;
use cp_zone::{ZoneCellRef, ZoneGrid};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

fn runner(content: &Arc<ContentDb>) -> SimRunner {
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(Arc::new(
        Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap(),
    ));
    sim.set_content(Arc::clone(content));
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

fn place(sim: &mut SimRunner, def: BuildingDefId, pos: Vec2, rot: f32) -> Result<(), String> {
    run(
        sim,
        SimCommand::Building(BuildingCommand::Place { def, pos, rot }),
    )
}

fn invalid_cells(sim: &SimRunner) -> Vec<ZoneCellRef> {
    let z = sim.world().resource::<ZoneGrid>();
    z.blocks()
        .flat_map(|(block, b)| {
            (0..b.cell_count() as u16).map(move |cell| ZoneCellRef { block, cell })
        })
        .filter(|&c| !z.is_valid(c))
        .collect()
}

fn tower_ids(sim: &SimRunner, def: BuildingDefId) -> Vec<BuildingId> {
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| b.def == def)
        .map(|(id, _)| id)
        .collect()
}

#[test]
fn a_free_standing_building_is_placed_powered_blocks_cells_and_survives_save_and_load() {
    let c = content();
    let tower = c.building_id("water_tower").unwrap();
    let turbine = c.building_id("coal_power_plant").unwrap();
    let mut sim = runner(&c);
    sim.world_mut().insert_resource(TileOwnership::only((2, 2)));
    let road = c.road_id("basic_2l").unwrap();
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &c.road(road).clone(),
            road,
            CubicBezier3::line(
                Vec3::new(5000.0, 0.0, 5000.0),
                Vec3::new(5200.0, 0.0, 5000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    let baseline = invalid_cells(&sim);

    // A coal plant on the road gives the power line something to carry.
    let spot = {
        let z = sim.world().resource::<ZoneGrid>();
        let (id, _) = z.blocks().next().unwrap();
        z.cell_footprint(ZoneCellRef {
            block: id,
            cell: 5 * 4 + 1,
        })
        .unwrap()
        .c
    };
    place(&mut sim, turbine, spot, 0.0).unwrap();

    // The tower stands in the field, 100 m from the road, no zone cells there.
    let far = Vec2::new(5100.0, 5100.0);
    place(&mut sim, tower, far, 0.4).unwrap();
    {
        let g = sim.world().resource::<BuildingGrid>();
        let (_, b) = g.iter().find(|(_, b)| b.def == tower).unwrap();
        assert!(b.lot().is_none() && b.access().is_none());
        assert_eq!((b.pos, b.rot), (far, 0.4));
    }
    // A line from the turbine to 30 m of the tower.
    run(
        &mut sim,
        SimCommand::Utility(UtilityCommand::BuildLink {
            kind: UtilityKind::Power,
            path: vec![
                Vec2::new(spot.x, spot.y + 14.0),
                Vec2::new(far.x, far.y - 30.0),
            ],
        }),
    )
    .unwrap();
    // A second tower 20 m off the road edge, where the zone cells are.
    let near = Vec2::new(5140.0, 5000.0 + 6.0 + 20.0 + 8.0);
    assert!(
        place(&mut sim, tower, Vec2::new(5140.0, 5003.0), 0.0)
            .unwrap_err()
            .contains("road"),
        "on the road"
    );
    place(&mut sim, tower, near, 0.0).unwrap();
    assert_eq!(tower_ids(&sim, tower).len(), 2);
    // A third tower in the field, beyond the line reach (80 m) and the power gap (64 m) of
    // everything else (T-181 raised both).
    let lone = Vec2::new(5190.0, 5190.0);
    place(&mut sim, tower, lone, 0.0).unwrap();

    for _ in 0..TICKS_PER_DAY {
        sim.tick_once();
    }
    let g = sim.world().resource::<BuildingGrid>();
    for (_, b) in g.iter() {
        assert_eq!(b.state, BuildingState::Active, "{:?}", b.def);
        assert!(
            !b.flags.has(BuildingFlags::NO_ROAD) && !b.flags.has(BuildingFlags::NO_PATH),
            "free-standing: no road flags"
        );
    }
    let far_tower = g
        .iter()
        .find(|(_, b)| b.def == tower && b.pos == far)
        .map(|(_, b)| b.flags)
        .unwrap();
    assert!(
        !far_tower.has(BuildingFlags::NO_POWER),
        "the line is 30 m away: powered"
    );
    let near_tower = g
        .iter()
        .find(|(_, b)| b.def == tower && b.pos == near)
        .map(|(_, b)| b.flags)
        .unwrap();
    assert!(
        !near_tower.has(BuildingFlags::NO_POWER),
        "the other tower is within the line reach and the gap of the powered ones"
    );
    let lone_tower = g
        .iter()
        .find(|(_, b)| b.def == tower && b.pos == lone)
        .map(|(_, b)| b.flags)
        .unwrap();
    assert!(
        lone_tower.has(BuildingFlags::NO_POWER),
        "out of reach of the line: the power flag means something"
    );

    // The cells under the near tower are invalid; the far one touches none.
    let blocked = invalid_cells(&sim);
    let under: Vec<_> = blocked
        .iter()
        .filter(|c| !baseline.contains(c))
        .filter(|&&c| {
            let o = sim
                .world()
                .resource::<ZoneGrid>()
                .cell_footprint(c)
                .unwrap();
            o.c.distance(near) < 16.0
        })
        .collect();
    assert!(
        under.len() >= 3,
        "cells under the 16 m tower: {}",
        under.len()
    );
    assert!(blocked.len() > baseline.len());
    let near_id = tower_ids(&sim, tower)
        .into_iter()
        .find(|&id| sim.world().resource::<BuildingGrid>().get(id).unwrap().pos == near)
        .unwrap();

    // Save → load: the same invalid cells and the same checksum.
    let bytes = save_runner(&sim, c.content_hash(), "flatlands").unwrap();
    let mut loaded = runner(&c);
    load_into_runner(&mut loaded, &bytes, c.content_hash()).unwrap();
    assert_eq!(invalid_cells(&loaded), blocked);
    assert_eq!(loaded.checksum(), sim.checksum());
    for _ in 0..200 {
        sim.tick_once();
        loaded.tick_once();
    }
    assert_eq!(invalid_cells(&loaded), blocked);
    assert_eq!(loaded.checksum(), sim.checksum());

    // Demolished: the cells are valid again.
    run(
        &mut sim,
        SimCommand::Building(BuildingCommand::Demolish { building: near_id }),
    )
    .unwrap();
    for _ in 0..4 {
        sim.tick_once();
    }
    assert_eq!(
        invalid_cells(&sim),
        baseline_with_turbine(&baseline, &sim, &blocked)
    );
}

/// The cells invalid before the tower, plus those the turbine's lot holds.
fn baseline_with_turbine(
    baseline: &[ZoneCellRef],
    sim: &SimRunner,
    blocked: &[ZoneCellRef],
) -> Vec<ZoneCellRef> {
    let z = sim.world().resource::<ZoneGrid>();
    // The cells under the buildings that stand free (a service at the road edge, T-267) stay
    // invalid too.
    let free: Vec<_> = sim
        .world()
        .resource::<cp_building::BuildingGrid>()
        .iter()
        .filter(|(_, b)| matches!(b.footprint, cp_building::Footprint::Free { .. }))
        .map(|(_, b)| b.footprint_polygon())
        .collect();
    let mut out: Vec<_> = blocked
        .iter()
        .copied()
        .filter(|c| {
            baseline.contains(c)
                || z.is_occupied(*c)
                || z.cell_footprint(*c).is_some_and(|o| {
                    free.iter()
                        .any(|q| cp_zone::geom::quads_overlap(q, &o.corners()))
                })
        })
        .collect();
    out.sort();
    out
}

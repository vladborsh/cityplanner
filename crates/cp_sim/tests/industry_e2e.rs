//! T-209: what the chain buildings make, day by day (cp_industry).

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::BuildingCommand;
use cp_building::{BuildingGrid, BuildingId};
use cp_citizen::CitizenGrid;
use cp_content::{CargoKind, ContentDb, ResourceKind};
use cp_core::Tick;
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{CommandError, CommandOutcome};
use cp_district::{AreaId, AreaKind, DistrictCommand};
use cp_industry::{IndustryProblem, IndustryState};
use cp_net::{Elevation, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::{GRID, MapMeta, RES_GRID, ResourceDeposits, ResourceMap, Terrain, TileOwnership};
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

/// A forestry area with its Main building (col 1); returns the area.
fn forest_area(sim: &mut SimRunner, c: &ContentDb) -> AreaId {
    let a = area(sim, AreaKind::Industry(ResourceKind::Forestry));
    let at = spot(sim, 1);
    place(sim, c, "forest_main", at).unwrap();
    a
}

fn building_of(sim: &SimRunner, c: &ContentDb, id: &str) -> BuildingId {
    let def = c.building_id(id).unwrap();
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .find(|(_, b)| b.def == def)
        .map(|(i, _)| i)
        .unwrap()
}

/// Gives `building` workers: `n[l]` of education level `l`.
fn staff(sim: &mut SimRunner, building: BuildingId, n: [usize; 4]) {
    let mut g = sim.world_mut().resource_mut::<CitizenGrid>();
    for (level, &count) in n.iter().enumerate() {
        // Top up what the job market let go; a household holds a few members only.
        let mut left = count.saturating_sub(usize::from(g.jobs_filled(building, level)));
        while left > 0 {
            let k = left.min(4);
            left -= k;
            let hid = g.add_household(building, 0, Tick(0), &vec![(9000, level as u8, false); k]);
            let members: Vec<_> = g
                .household(hid)
                .unwrap()
                .members
                .iter()
                .flatten()
                .copied()
                .collect();
            assert_eq!(members.len(), k);
            for m in members {
                g.set_job(m, building, level as u8);
            }
        }
    }
}

fn days(sim: &mut SimRunner, n: u64) {
    for _ in 0..n * TICKS_PER_DAY {
        sim.tick_once();
    }
}

fn stock(sim: &SimRunner, b: BuildingId, c: CargoKind) -> f32 {
    sim.world().resource::<IndustryState>().stock_of(b, c)
}

fn problem(sim: &SimRunner, b: BuildingId) -> IndustryProblem {
    sim.world().resource::<IndustryState>().problem_of(b)
}

#[test]
fn a_sawmill_fills_its_store_in_proportion_to_its_workers_and_nothing_without() {
    let c = content();
    let mut sim = world(&c);
    forest_area(&mut sim, &c);
    let at = spot(&sim, 7);
    place(&mut sim, &c, "forest_extractor_s", at).unwrap();
    let saw = building_of(&sim, &c, "forest_extractor_s");

    // Nobody works there: nothing is made.
    days(&mut sim, 1);
    assert_eq!(stock(&sim, saw, CargoKind::Logs), 0.0);

    // 4 of its 8 jobs are filled: half of the 40 a day, on forest of richness 1.
    staff(&mut sim, saw, [4, 0, 0, 0]);
    days(&mut sim, 1);
    let logs = stock(&sim, saw, CargoKind::Logs);
    assert!((logs - 20.0).abs() < 0.01, "{logs}");
    assert_eq!(problem(&sim, saw), IndustryProblem::None);
}

#[test]
fn a_processor_without_input_says_so_and_with_input_makes_planks() {
    let c = content();
    let mut sim = world(&c);
    forest_area(&mut sim, &c);
    let at = spot(&sim, 7);
    place(&mut sim, &c, "forest_processor_a", at).unwrap();
    let p = building_of(&sim, &c, "forest_processor_a");
    // 4 of its 9 jobs are filled (the educated jobs are shuffled by the job market).
    staff(&mut sim, p, [4, 0, 0, 0]);
    let f = 4.0 / 9.0;

    days(&mut sim, 1);
    assert_eq!(problem(&sim, p), IndustryProblem::NoInput);
    staff(&mut sim, p, [4, 0, 0, 0]);
    assert_eq!(stock(&sim, p, CargoKind::Planks), 0.0);

    sim.world_mut()
        .resource_mut::<IndustryState>()
        .stock
        .insert(p, vec![(CargoKind::Logs, 100.0)]);
    staff(&mut sim, p, [4, 0, 0, 0]);
    days(&mut sim, 1);
    assert_eq!(problem(&sim, p), IndustryProblem::None);
    assert!((stock(&sim, p, CargoKind::Planks) - 40.0 * f).abs() < 0.01);
    assert!((stock(&sim, p, CargoKind::Logs) - (100.0 - 60.0 * f)).abs() < 0.01);

    // Less input than a full day: it works as far as the input goes.
    sim.world_mut()
        .resource_mut::<IndustryState>()
        .stock
        .insert(p, vec![(CargoKind::Logs, 10.0)]);
    staff(&mut sim, p, [4, 0, 0, 0]);
    days(&mut sim, 1);
    // 10 logs are less than a day's work: all of them are used, and the planks that come of
    // them are fewer than a full day's.
    assert!(stock(&sim, p, CargoKind::Logs) < 0.01);
    let made = stock(&sim, p, CargoKind::Planks);
    assert!(made > 0.0 && made < 40.0 * f - 0.01, "{made}");
}

#[test]
fn a_mine_uses_up_the_ground_and_is_depleted() {
    let c = content();
    let mut sim = world(&c);
    let _ = area(&mut sim, AreaKind::Industry(ResourceKind::Ore));
    let at = spot(&sim, 1);
    place(&mut sim, &c, "ore_main", at).unwrap();
    let at = spot(&sim, 7);
    place(&mut sim, &c, "ore_extractor_s", at).unwrap();
    let mine = building_of(&sim, &c, "ore_extractor_s");
    staff(&mut sim, mine, [6, 2, 0, 0]);
    // 3 ore a cell: the lot has a few cells, far less than a day's 40.
    let map = sim.view().terrain.as_ref().unwrap().resources().clone();
    sim.world_mut()
        .insert_resource(ResourceDeposits::from_map(&map, 3.0));
    let before: u32 = (0..RES_GRID * RES_GRID)
        .map(|i| {
            u32::from(sim.world().resource::<ResourceDeposits>().remaining(
                ResourceKind::Ore,
                i % RES_GRID,
                i / RES_GRID,
            ))
        })
        .sum();

    days(&mut sim, 1);
    let ore = stock(&sim, mine, CargoKind::Ore);
    assert!(ore > 0.0 && ore < 40.0, "{ore}");
    let after: u32 = (0..RES_GRID * RES_GRID)
        .map(|i| {
            u32::from(sim.world().resource::<ResourceDeposits>().remaining(
                ResourceKind::Ore,
                i % RES_GRID,
                i / RES_GRID,
            ))
        })
        .sum();
    assert_eq!(before - after, ore as u32, "taken from the ground");

    // Run it dry.
    days(&mut sim, 3);
    assert_eq!(problem(&sim, mine), IndustryProblem::Depleted);
    let total = stock(&sim, mine, CargoKind::Ore);
    days(&mut sim, 1);
    assert_eq!(stock(&sim, mine, CargoKind::Ore), total);
}

#[test]
fn an_area_earns_xp_and_rises_a_level_at_the_threshold() {
    let c = content();
    let mut sim = world(&c);
    let a = forest_area(&mut sim, &c);
    let at = spot(&sim, 7);
    place(&mut sim, &c, "forest_extractor_s", at).unwrap();
    let saw = building_of(&sim, &c, "forest_extractor_s");
    staff(&mut sim, saw, [8, 0, 0, 0]);
    let need = c.balance().industry_levels[0];
    sim.world_mut()
        .resource_mut::<cp_district::Areas>()
        .get_mut(a)
        .unwrap()
        .xp = need - 10;
    days(&mut sim, 1);
    let area = sim.view().areas.get(a).unwrap().clone();
    assert_eq!(area.xp, need + 30);
    assert_eq!(area.level, 2);
}

#[test]
fn a_full_store_stops_the_work_and_a_missing_main_building_stops_everything() {
    let c = content();
    let mut sim = world(&c);
    forest_area(&mut sim, &c);
    let at = spot(&sim, 7);
    place(&mut sim, &c, "forest_extractor_s", at).unwrap();
    let saw = building_of(&sim, &c, "forest_extractor_s");
    staff(&mut sim, saw, [8, 0, 0, 0]);
    sim.world_mut()
        .resource_mut::<IndustryState>()
        .stock
        .insert(saw, vec![(CargoKind::Logs, 195.0)]);
    days(&mut sim, 1);
    // The store is 200: five more fit, the rest is not made.
    assert!((stock(&sim, saw, CargoKind::Logs) - 200.0).abs() < 0.01);
    days(&mut sim, 1);
    assert_eq!(problem(&sim, saw), IndustryProblem::Full);

    let main = building_of(&sim, &c, "forest_main");
    sim.world_mut().resource_mut::<BuildingGrid>().remove(main);
    sim.world_mut()
        .resource_mut::<IndustryState>()
        .stock
        .insert(saw, vec![(CargoKind::Logs, 10.0)]);
    days(&mut sim, 1);
    assert_eq!(problem(&sim, saw), IndustryProblem::NoMain);
    assert_eq!(stock(&sim, saw, CargoKind::Logs), 10.0);
}

#[test]
fn industry_is_deterministic_and_part_of_the_checksum() {
    let play = |staffed: bool| {
        let c = content();
        let mut sim = world(&c);
        forest_area(&mut sim, &c);
        let at = spot(&sim, 7);
        place(&mut sim, &c, "forest_extractor_s", at).unwrap();
        if staffed {
            let saw = building_of(&sim, &c, "forest_extractor_s");
            staff(&mut sim, saw, [5, 0, 0, 0]);
        }
        days(&mut sim, 2);
        sim
    };
    let (mut a, mut b, mut c) = (play(true), play(true), play(false));
    assert_eq!(a.checksum(), b.checksum());
    assert_ne!(a.checksum(), c.checksum());
}

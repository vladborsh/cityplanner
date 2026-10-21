//! BUG-110: a service building being moved onto grown houses demolishes them with the move, the
//! way a `Place` does (T-252): same entity, the houses gone, their households rehoused. The ghost
//! and the command name the same houses. Another service under the new spot still blocks.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    Building, BuildingCommand, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint,
    Lot,
};
use cp_citizen::{CitizenGrid, DAYS_PER_YEAR, HouseholdId};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome, Tick};
use cp_net::{Elevation, NetGraph};
use cp_sim::{SimCommand, SimRunner};
use cp_zone::{BLOCK_DEPTH, ZoneBlockId, ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// A 200 m street; everything stands on the first zone block beside it.
fn town() -> (SimRunner, ZoneBlockId) {
    let db = content();
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(&db));
    sim.world_mut()
        .insert_resource(cp_terrain::TileOwnership::only((1, 1)));
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
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance =
        cp_core::Cost::from_units(1_000_000.0);
    let block = sim
        .world()
        .resource::<ZoneGrid>()
        .blocks()
        .next()
        .unwrap()
        .0;
    (sim, block)
}

fn def(sim: &SimRunner, name: &str) -> cp_content::BuildingDefId {
    sim.world()
        .resource::<cp_content::SimContent>()
        .0
        .building_id(name)
        .unwrap()
}

/// A ready building of `name` on the 2 x 2 lot from column `col`, holding its cells.
fn stand(sim: &mut SimRunner, block: ZoneBlockId, name: &str, col: u16) -> BuildingId {
    let lot = Lot {
        block,
        col,
        w: 2,
        d: 2,
    };
    let def = def(sim, name);
    let (pos, rot, access) = {
        let w = sim.world();
        cp_building::place(
            w.resource::<ZoneGrid>(),
            w.resource::<NetGraph>(),
            block,
            &lot,
        )
        .unwrap()
    };
    let id = sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def,
            footprint: Footprint::Lot(lot),
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos,
            rot,
            access: Some(access),
            flags: BuildingFlags(0),
            level_progress: 0.0,
        });
    let cells: Vec<ZoneCellRef> = lot.cells().collect();
    sim.world_mut().resource_mut::<ZoneGrid>().occupy(&cells);
    id
}

/// A household of `n` adults living in `home`, already arrived.
fn household(sim: &mut SimRunner, home: BuildingId, n: usize) -> HouseholdId {
    let now = sim.world().resource::<Tick>().0;
    let mut g = sim.world_mut().resource_mut::<CitizenGrid>();
    g.ensure_derived();
    g.add_household(home, 0, Tick(now), &vec![(30 * DAYS_PER_YEAR, 1, false); n])
}

/// The centre of the cell in column `col`, second row: where the cursor points.
fn at(sim: &SimRunner, block: ZoneBlockId, col: u16) -> Vec2 {
    sim.world()
        .resource::<ZoneGrid>()
        .cell_footprint(ZoneCellRef {
            block,
            cell: col * BLOCK_DEPTH + 1,
        })
        .unwrap()
        .c
}

fn place(sim: &mut SimRunner, name: &str, pos: Vec2) -> Result<(), CommandError> {
    let def = def(sim, name);
    sim.push_command(
        SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
        sim.tick(),
    );
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last().unwrap().result.clone().map(|_| ())
}

fn mv(sim: &mut SimRunner, building: BuildingId, pos: Vec2) -> Result<(), CommandError> {
    sim.push_command(
        SimCommand::Building(BuildingCommand::Move {
            building,
            pos,
            rot: 0.0,
        }),
        sim.tick(),
    );
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last().unwrap().result.clone().map(|_| ())
}

fn clinic_id(sim: &SimRunner) -> BuildingId {
    let clinic = def(sim, "clinic");
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .find(|(_, b)| b.def == clinic)
        .expect("the clinic stands")
        .0
}

#[test]
fn moving_a_clinic_onto_two_houses_demolishes_them_and_the_ghost_names_the_same() {
    let (mut sim, block) = town();
    let house_a = stand(&mut sim, block, "house_low_1", 0);
    let house_b = stand(&mut sim, block, "house_low_1", 2);
    let vacant = stand(&mut sim, block, "house_low_1", 14);
    let evicted = household(&mut sim, house_a, 3);
    let far = at(&sim, block, 8).lerp(at(&sim, block, 9), 0.5);
    place(&mut sim, "clinic", far).expect("a clinic on free ground");
    let clinic = clinic_id(&sim);
    let dest = at(&sim, block, 1).lerp(at(&sim, block, 2), 0.5);

    // What the ghost shows: the same function the command uses.
    let shown = {
        let w = sim.world();
        let db = &w.resource::<cp_content::SimContent>().0;
        let c = cp_building::validate_move_over(
            db,
            w.resource::<ZoneGrid>(),
            w.resource::<NetGraph>(),
            w.resource::<BuildingGrid>(),
            None,
            w.get_resource::<cp_terrain::TileOwnership>(),
            def(&sim, "clinic"),
            dest,
            0.0,
            clinic,
        )
        .expect("the spot is allowed");
        c.demolish
    };
    assert_eq!(shown, vec![house_a, house_b]);

    mv(&mut sim, clinic, dest).expect("the move over two houses");
    let w = sim.world();
    let buildings = w.resource::<BuildingGrid>();
    assert!(buildings.get(house_a).is_none() && buildings.get(house_b).is_none());
    assert!(buildings.get(vacant).is_some(), "other houses stay");
    let c = buildings.get(clinic).expect("the same clinic, same id");
    assert!(c.pos.distance(dest) < 30.0, "it stands on the new spot");
    let g = w.resource::<CitizenGrid>();
    assert_eq!(g.household(evicted).unwrap().home, vacant, "rehoused");
}

#[test]
fn moving_a_clinic_onto_a_school_never_demolishes_the_school() {
    let (mut sim, block) = town();
    let house = stand(&mut sim, block, "house_low_1", 0);
    let school_at = at(&sim, block, 6).lerp(at(&sim, block, 7), 0.5);
    place(&mut sim, "elementary_school", school_at).expect("a school");
    let school = def(&sim, "elementary_school");
    let far = at(&sim, block, 12).lerp(at(&sim, block, 13), 0.5);
    place(&mut sim, "clinic", far).expect("a clinic");
    let clinic = clinic_id(&sim);
    let n = sim.world().resource::<BuildingGrid>().len();
    // Dead centre on the school: a service is no obstacle to clear.
    let centre = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .find(|(_, b)| b.def == school)
        .unwrap()
        .1
        .pos;
    // The placement snaps to the street frontage, so the clinic lands in the free slot beside
    // the school (it cannot be refused by aiming at it); the school is never in the ghost's
    // demolish list and stands untouched.
    let shown = {
        let w = sim.world();
        cp_building::validate_move_over(
            &w.resource::<cp_content::SimContent>().0,
            w.resource::<ZoneGrid>(),
            w.resource::<NetGraph>(),
            w.resource::<BuildingGrid>(),
            None,
            w.get_resource::<cp_terrain::TileOwnership>(),
            def(&sim, "clinic"),
            centre,
            0.0,
            clinic,
        )
    };
    if let Ok(c) = shown {
        assert!(c.demolish.is_empty(), "no service in the ghost's demolish list");
    }
    mv(&mut sim, clinic, centre).expect("the clinic is set beside the school");
    let buildings = sim.world().resource::<BuildingGrid>();
    assert_eq!(buildings.len(), n, "nothing demolished");
    assert!(buildings.get(house).is_some());
    let (_, s) = buildings.iter().find(|(_, b)| b.def == school).unwrap();
    let c = buildings.get(clinic).unwrap();
    assert!(!cp_zone::geom::quads_overlap(
        &s.footprint_polygon(),
        &c.footprint_polygon()
    ));
}

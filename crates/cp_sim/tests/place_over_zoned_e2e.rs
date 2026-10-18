//! T-252: a service building may be placed over zoned buildings. They are demolished the way the
//! bulldozer takes them (the zone cells keep their type), the households of a demolished home
//! move to the nearest vacant home or leave the city, the workers of a demolished workplace lose
//! the job. Another service or a road still blocks; the result replays.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    Building, BuildingCommand, BuildingError, BuildingFlags, BuildingGrid, BuildingId,
    BuildingState, Footprint, Lot,
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

struct Scene {
    sim: SimRunner,
    house: BuildingId,
    shop: BuildingId,
    vacant: BuildingId,
    evicted: HouseholdId,
    worker: cp_citizen::CitizenId,
}

/// A house (columns 0-1, a household of three) and a shop (2-3, one worker who lives in a house
/// further on, 10-11) side by side, a vacant house at 14-15; then a clinic over columns 1-2.
fn clinic_over_house_and_shop() -> Scene {
    let (mut sim, block) = town();
    let house = stand(&mut sim, block, "house_low_1", 0);
    let shop = stand(&mut sim, block, "com_low_1", 2);
    let home = stand(&mut sim, block, "house_low_1", 10);
    let vacant = stand(&mut sim, block, "house_low_1", 14);
    let evicted = household(&mut sim, house, 3);
    let worker_home = household(&mut sim, home, 1);
    let worker = {
        let g = sim.world().resource::<CitizenGrid>();
        g.household(worker_home)
            .unwrap()
            .member_ids()
            .next()
            .unwrap()
    };
    sim.world_mut()
        .resource_mut::<CitizenGrid>()
        .set_job(worker, shop, 0);
    let pos = at(&sim, block, 1).lerp(at(&sim, block, 2), 0.5);
    place(&mut sim, "clinic", pos).expect("a clinic over a house and a shop");
    Scene {
        sim,
        house,
        shop,
        vacant,
        evicted,
        worker,
    }
}

#[test]
fn a_clinic_over_two_zoned_buildings_demolishes_them_and_rehouses_the_residents() {
    let mut s = clinic_over_house_and_shop();
    {
        let w = s.sim.world();
        let buildings = w.resource::<BuildingGrid>();
        assert!(buildings.get(s.house).is_none(), "the house is gone");
        assert!(buildings.get(s.shop).is_none(), "the shop is gone");
        let clinic = def(&s.sim, "clinic");
        let (_, c) = buildings
            .iter()
            .find(|(_, b)| b.def == clinic)
            .expect("the clinic stands");
        assert_eq!(c.state, BuildingState::Active);
        let lot = c.lot().unwrap();
        assert_eq!((lot.col, lot.w), (1, 2), "on columns 1-2");
        // The household moved into the vacant house at once.
        let g = w.resource::<CitizenGrid>();
        let h = g
            .household(s.evicted)
            .expect("the household stays in the city");
        assert_eq!(h.home, s.vacant);
        assert_eq!(g.households_in(s.vacant), 1);
        for cid in h.member_ids() {
            assert_eq!(g.citizen(cid).unwrap().home, Some(s.vacant));
        }
        // The cells of the house that the clinic does not cover are free again, still zoned.
        let zones = w.resource::<ZoneGrid>();
        let block = lot.block;
        let freed = ZoneCellRef { block, cell: 0 };
        assert!(!zones.is_occupied(freed));
    }
    // The purge a phase later takes the job of the demolished shop.
    s.sim.tick_once();
    let g = s.sim.world().resource::<CitizenGrid>();
    assert_eq!(g.citizen(s.worker).unwrap().work, None, "a jobseeker now");
    assert!(g.household(s.evicted).is_some());
    assert_eq!(g.citizen_count(), 4, "nobody left the city");
}

#[test]
fn without_a_vacant_home_the_evicted_household_leaves_the_city() {
    let (mut sim, block) = town();
    let house = stand(&mut sim, block, "house_low_1", 0);
    let full = stand(&mut sim, block, "house_low_1", 6);
    let evicted = household(&mut sim, house, 2);
    household(&mut sim, full, 1);
    let pos = at(&sim, block, 0).lerp(at(&sim, block, 1), 0.5);
    place(&mut sim, "clinic", pos).expect("a clinic over the house");
    sim.tick_once();
    let g = sim.world().resource::<CitizenGrid>();
    assert!(g.household(evicted).is_none(), "the household left");
    assert_eq!(g.citizen_count(), 1);
    assert!(sim.world().resource::<BuildingGrid>().get(house).is_none());
}

#[test]
fn another_service_building_or_a_road_still_blocks() {
    let (mut sim, block) = town();
    let pos = at(&sim, block, 4).lerp(at(&sim, block, 5), 0.5);
    place(&mut sim, "clinic", pos).expect("the first clinic");
    let count = |sim: &SimRunner| sim.world().resource::<BuildingGrid>().len();
    let before = count(&sim);
    let err = place(&mut sim, "police_station", pos).unwrap_err();
    assert!(
        matches!(&err, CommandError::Rejected { reason, .. } if reason == &BuildingError::Blocked.to_string()),
        "{err:?}"
    );
    assert_eq!(count(&sim), before, "nothing demolished, nothing placed");
    // On the road: no lot there.
    let road = Vec2::new(3100.0, 3000.0);
    assert!(place(&mut sim, "clinic", road).is_err());
    assert_eq!(count(&sim), before);
}

#[test]
fn placing_over_zoned_buildings_is_deterministic() {
    let snapshot = |s: &Scene| {
        let w = s.sim.world();
        (
            w.resource::<BuildingGrid>().clone(),
            w.resource::<CitizenGrid>().clone(),
            w.resource::<ZoneGrid>().valid_cells(),
        )
    };
    let (mut a, mut b) = (clinic_over_house_and_shop(), clinic_over_house_and_shop());
    for _ in 0..50 {
        a.sim.tick_once();
        b.sim.tick_once();
    }
    let (ba, ca, za) = snapshot(&a);
    let (bb, cb, zb) = snapshot(&b);
    assert!(ba == bb && ca == cb && za == zb);
}

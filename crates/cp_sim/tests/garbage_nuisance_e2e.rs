//! T-290: a garbage facility is a nuisance to the homes near it. Within `garbage_nuisance_m`
//! (8 cells, 64 m) of the footprint the land takes up to `land_nuisance_garbage` off, falling to
//! nothing at that distance; industry, shops and offices do not feel it; the land value grid
//! (the overlay) shows the dip and the inspector of an affected home says so.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint};
use cp_content::{ContentDb, ZoneType};
use cp_core::Tick;
use cp_core::curve::CubicBezier3;
use cp_economy::{CELL_M, LandGrids};
use cp_net::{Elevation, NetGraph};
use cp_sim::{Inspection, Selection, SimRunner};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
/// The middle of cell (47, 47) of the 64 m land grid: the landfill stands there.
const C0: Vec2 = Vec2::new(47.5 * CELL_M, 47.5 * CELL_M);

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

fn put(sim: &mut SimRunner, db: &ContentDb, def: &str, pos: Vec2) -> BuildingId {
    let def = db.building_id(def).unwrap();
    let d = db.building(def);
    sim.world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def,
            footprint: Footprint::Free {
                w: d.lot.0,
                d: d.lot.1,
            },
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos,
            rot: 0.0,
            access: None,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        })
}

/// Runs the sim across two land value passes (every 256 ticks).
fn settle(sim: &mut SimRunner) {
    for _ in 0..600 {
        sim.tick_once();
    }
}

/// A street through the middle of the cell gives the land +30 (base 20), so that the dips do not
/// run into the floor of 0. The landfill is an industrial building and pollutes like one, the
/// same in a town with and without it.
struct Town {
    sim: SimRunner,
    /// Homes 0, 1 and 2 cells east of the landfill's cell (in the towns with homes).
    homes: Vec<BuildingId>,
    /// A factory one cell east of the landfill and a shop one cell west (in the other towns).
    factory: Option<BuildingId>,
    shop: Option<BuildingId>,
    landfill: Option<BuildingId>,
}

fn town(landfill: bool, homes: bool) -> Town {
    let db = content();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(&db));
    let road = db.road_id("basic_2l").unwrap();
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &db.road(road).clone(),
            road,
            CubicBezier3::line(
                Vec3::new(C0.x - 300.0, 0.0, C0.y),
                Vec3::new(C0.x + 500.0, 0.0, C0.y),
            ),
            Elevation::Ground,
        )
        .unwrap();
    let landfill = landfill.then(|| put(&mut sim, &db, "landfill", C0));
    let mut t = Town {
        sim,
        homes: Vec::new(),
        factory: None,
        shop: None,
        landfill,
    };
    if homes {
        for n in 0..3 {
            let at = C0 + Vec2::new(n as f32 * CELL_M, 0.0);
            let id = put(&mut t.sim, &db, "house_low_1", at);
            t.homes.push(id);
        }
    } else {
        t.factory = Some(put(&mut t.sim, &db, "ind_1", C0 + Vec2::new(CELL_M, 0.0)));
        t.shop = Some(put(
            &mut t.sim,
            &db,
            "com_low_1",
            C0 + Vec2::new(-CELL_M, 0.0),
        ));
    }
    settle(&mut t.sim);
    t
}

fn pos(t: &Town, id: BuildingId) -> Vec2 {
    t.sim
        .world()
        .resource::<BuildingGrid>()
        .get(id)
        .unwrap()
        .pos
}

fn value(t: &Town, zone: ZoneType, id: BuildingId) -> f32 {
    t.sim
        .world()
        .resource::<LandGrids>()
        .value_for(zone, pos(t, id))
}

#[test]
fn a_landfill_takes_land_value_off_the_homes_near_it_by_distance() {
    let clean = town(false, true);
    let mut dirty = town(true, true);
    let b = content().balance().clone();
    assert_eq!(
        (b.land_nuisance_garbage, b.garbage_nuisance_m),
        (24.0, 64.0)
    );
    // The landfill is a 4x4 lot: half of it is 16 m, and the nuisance counts from its edge.
    let half = 16.0;
    let dip = |cells: f32| {
        (b.land_nuisance_garbage * (1.0 - (cells * CELL_M - half).max(0.0) / b.garbage_nuisance_m))
            .max(0.0)
    };
    // 0 cells: the full 24; 1 cell (64 m): 24 * (1 - 48 / 64) = 6; 2 cells (128 m): nothing.
    assert!((dip(0.0) - 24.0).abs() < 1e-4 && (dip(1.0) - 6.0).abs() < 1e-4);
    assert_eq!(dip(2.0), 0.0);
    let mut before_after = Vec::new();
    for n in 0..3 {
        let before = value(&clean, ZoneType::ResLow, clean.homes[n]);
        let after = value(&dirty, ZoneType::ResLow, dirty.homes[n]);
        before_after.push((before, after));
        assert!(
            before > 24.0,
            "the street keeps the land off the floor: {before}"
        );
    }
    // Pollution of the landfill itself (an industrial building) is in the dirty town only, so
    // compare what the nuisance grid took off with the formula, and the land value with the grid.
    let grids = dirty.sim.world().resource::<LandGrids>();
    for n in 0..3 {
        let at = pos(&dirty, dirty.homes[n]);
        assert!(
            (grids.garbage_at(at) - dip(n as f32)).abs() < 1e-3,
            "home {n}: took {} expected {}",
            grids.garbage_at(at),
            dip(n as f32)
        );
        // Homes see the land with the dip; the grid (the overlay) shows the same.
        assert_eq!(
            grids.value_for(ZoneType::ResLow, at),
            grids.value_at(at),
            "a home reads the land value grid"
        );
    }
    // Removing the landfill restores everything: the nuisance grid is empty and every home has
    // the land value of the clean town again (pollution of the landfill gone as well).
    let landfill = dirty.landfill.unwrap();
    dirty
        .sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .remove(landfill);
    settle(&mut dirty.sim);
    let grids = dirty.sim.world().resource::<LandGrids>();
    assert!(grids.garbage.iter().all(|v| *v == 0.0));
    for n in 0..3 {
        let a = value(&clean, ZoneType::ResLow, clean.homes[n]);
        let b = value(&dirty, ZoneType::ResLow, dirty.homes[n]);
        assert!((a - b).abs() < 1e-3, "home {n}: {a} vs {b} after removal");
    }
    // Further out (128 m) a home is as it was with the landfill standing, the nuisance aside.
    let (b0, a0) = before_after[0];
    let (b2, a2) = before_after[2];
    assert!(
        b0 - a0 > 24.0 - 1.0,
        "home 0 lost the dip and the landfill's smoke: {b0} -> {a0}"
    );
    assert!(b2 >= a2, "{b2} -> {a2}");
}

#[test]
fn industry_and_shops_next_to_a_landfill_do_not_feel_the_nuisance() {
    let clean = town(false, false);
    let dirty = town(true, false);
    let (cf, cs) = (clean.factory.unwrap(), clean.shop.unwrap());
    let (df, ds) = (dirty.factory.unwrap(), dirty.shop.unwrap());
    let grids = dirty.sim.world().resource::<LandGrids>();
    // The nuisance is on their cells (the land value grid, the overlay, shows it) ...
    for id in [df, ds] {
        assert!(grids.garbage_at(pos(&dirty, id)) > 0.0);
    }
    // ... but what a factory or a shop reads is the land the cell has without it: the landfill's
    // smoke aside (a landfill pollutes like any industrial building, nuisance or not), which
    // the clean town does not have; so compare against the same town with the nuisance added
    // back by hand.
    for (zone, id) in [(ZoneType::Industrial, df), (ZoneType::ComLow, ds)] {
        let at = pos(&dirty, id);
        let read = grids.value_for(zone, at);
        assert_eq!(read, grids.value_at(at) + grids.garbage_at(at), "{zone:?}");
        assert!(read > grids.value_at(at));
    }
    let _ = (cf, cs);
}

#[test]
fn the_inspector_of_a_home_near_a_landfill_says_so_and_a_far_home_and_workplaces_do_not() {
    let homes = town(true, true);
    let rows = |t: &Town, id| match t.sim.inspect(Selection::Building(id)) {
        Some(Inspection::Building(b)) => Inspection::Building(b).rows(),
        other => panic!("{other:?}"),
    };
    let nuisance = |t: &Town, id| {
        rows(t, id)
            .into_iter()
            .find(|(k, _)| k == "Nuisance")
            .map(|(_, v)| v)
    };
    assert_eq!(
        nuisance(&homes, homes.homes[0]).as_deref(),
        Some("Near a landfill")
    );
    assert_eq!(
        nuisance(&homes, homes.homes[1]).as_deref(),
        Some("Near a landfill")
    );
    assert_eq!(
        nuisance(&homes, homes.homes[2]),
        None,
        "128 m: out of reach"
    );
    let work = town(true, false);
    assert_eq!(
        nuisance(&work, work.factory.unwrap()),
        None,
        "industry is fine next to it"
    );
    assert_eq!(nuisance(&work, work.shop.unwrap()), None);
}

#[test]
fn the_nuisance_field_is_the_same_every_run() {
    let a = town(true, true);
    let b = town(true, true);
    let ga = a.sim.world().resource::<LandGrids>();
    let gb = b.sim.world().resource::<LandGrids>();
    assert_eq!(ga, gb);
    assert!(ga.garbage.iter().any(|v| *v > 0.0));
}

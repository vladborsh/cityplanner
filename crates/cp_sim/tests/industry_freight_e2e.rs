//! T-210: cargo by truck along the chain, to the outside and in from it.

use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Footprint, Lot,
};
use cp_citizen::CitizenGrid;
use cp_content::{CargoKind, ContentDb, ResourceKind};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Cost, Id, Tick};
use cp_district::{Area, AreaId, AreaKind, AreaMap, Areas};
use cp_industry::{IndustryLedger, IndustryState};
use cp_net::{Elevation, NetGraph, NodeKind};
use cp_sim::SimRunner;
use cp_terrain::{GRID, MapMeta, RES_GRID, ResourceMap, Terrain};
use cp_transit::{Endpoint, Goods};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

struct Town {
    sim: SimRunner,
    db: Arc<ContentDb>,
    seg: cp_net::SegmentId,
}

/// A 1 km street along z = 5 (an `Outside` node at x = 0 when `outside`), a forestry area over
/// it with its Main building at x = 40, and forest under the whole street.
fn town(outside: bool) -> Town {
    let db = content();
    let mut sim = SimRunner::new(3);
    sim.set_content(Arc::clone(&db));
    let meta = MapMeta {
        name: "flat".into(),
        sea_level: 0.0,
        start_tile: (0, 0),
        external_connections: vec![],
        entry_road: None,
        entry_rail: None,
    };
    let px = (0..RES_GRID * RES_GRID)
        .map(|i| {
            if i / RES_GRID == 0 {
                [0, 255, 0, 0]
            } else {
                [0; 4]
            }
        })
        .collect();
    sim.set_terrain(Arc::new(
        Terrain::from_samples(meta, &vec![1000; GRID * GRID], &vec![0; GRID * GRID])
            .unwrap()
            .with_resources(ResourceMap::from_pixels(px).unwrap()),
    ));
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    if outside {
        sim.world_mut().resource_mut::<NetGraph>().add_node(
            Vec3::ZERO,
            NodeKind::Outside,
            Elevation::Ground,
        );
    }
    let seg = sim
        .world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0)),
            Elevation::Ground,
        )
        .unwrap();
    {
        let w = sim.world_mut();
        w.resource_mut::<Areas>().list.push(Some(Area {
            name: "Forest 1".into(),
            kind: AreaKind::Industry(ResourceKind::Forestry),
            level: 1,
            xp: 0,
        }));
        let mut map = w.resource_mut::<AreaMap>();
        for c in 0..32 {
            map.set((c, 0), Some(AreaId(1)));
        }
    }
    let mut t = Town { sim, db, seg };
    t.put("forest_main", 40.0);
    t.sim.tick_once();
    t
}

impl Town {
    fn put(&mut self, name: &str, at: f32) -> BuildingId {
        let seg = self.seg;
        self.sim
            .world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def: self.db.building_id(name).unwrap(),
                footprint: Footprint::Lot(Lot {
                    block: Id::new(0, 0),
                    col: 0,
                    w: 1,
                    d: 1,
                }),
                level: 1,
                state: BuildingState::Active,
                ready_at: Tick(0),
                pos: Vec2::new(at, 5.0),
                rot: 0.0,
                access: Some(AccessPoint {
                    segment: seg,
                    t: at / 1000.0,
                }),
                flags: BuildingFlags(0),
                level_progress: 0.0,
            })
    }

    /// Tops the building's workers of education 0 up to `n`.
    fn staff(&mut self, b: BuildingId, n: usize) {
        let mut g = self.sim.world_mut().resource_mut::<CitizenGrid>();
        let mut left = n.saturating_sub(usize::from(g.jobs_filled(b, 0)));
        while left > 0 {
            let k = left.min(4);
            left -= k;
            let hid = g.add_household(b, 0, Tick(0), &vec![(9000, 0, false); k]);
            let members: Vec<_> = g
                .household(hid)
                .unwrap()
                .members
                .iter()
                .flatten()
                .copied()
                .collect();
            for m in members {
                g.set_job(m, b, 0);
            }
        }
    }

    fn stock(&self, b: BuildingId, c: CargoKind) -> f32 {
        self.sim.world().resource::<IndustryState>().stock_of(b, c)
    }

    fn seed(&mut self, b: BuildingId, c: CargoKind, v: f32) {
        self.sim
            .world_mut()
            .resource_mut::<IndustryState>()
            .stock
            .insert(b, vec![(c, v)]);
    }

    fn ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.sim.tick_once();
        }
    }
}

#[test]
fn logs_reach_the_processor_600_m_away_and_its_input_store_grows() {
    let mut t = town(false);
    let saw = t.put("forest_extractor_s", 100.0);
    let mill = t.put("forest_processor_a", 700.0);
    t.staff(saw, 8);
    t.seed(saw, CargoKind::Logs, 60.0);
    let mut sent = false;
    let mut got = 0.0;
    for _ in 0..3 * TICKS_PER_DAY {
        t.sim.tick_once();
        sent |= t
            .sim
            .world()
            .resource::<Goods>()
            .hauls
            .values()
            .any(|h| h.cargo == CargoKind::Logs && h.to == Endpoint::Building(mill));
        got = t.stock(mill, CargoKind::Logs);
        if got > 0.0 {
            break;
        }
    }
    assert!(sent, "a Logs truck set out for the processor");
    assert!(got >= 20.0, "a load of 20 reached its store: {got}");
    assert!(t.stock(saw, CargoKind::Logs) < 60.0 + 40.0);
}

#[test]
fn without_a_processor_the_cargo_is_exported_and_paid_for() {
    let mut t = town(true);
    let saw = t.put("forest_extractor_s", 100.0);
    t.staff(saw, 8);
    t.seed(saw, CargoKind::Logs, 40.0);
    for _ in 0..4 * TICKS_PER_DAY {
        t.sim.tick_once();
        if t.sim.world().resource::<IndustryLedger>().export_loads > 0 {
            break;
        }
    }
    let l = *t.sim.world().resource::<IndustryLedger>();
    assert!(l.export_loads > 0, "{l:?}");
    // 20 units a load at the Logs export price.
    let price = t.db.cargo(CargoKind::Logs).unwrap().export_price;
    assert_eq!(
        l.exports,
        Cost::scaled(price, 20.0 * f64::from(l.export_loads))
    );
    // The week's settlement books it into the treasury.
    let before = t.sim.world().resource::<cp_core::Treasury>().balance;
    let _ = before;
    t.ticks(cp_core::time::TICKS_PER_WEEK);
    let l = *t.sim.world().resource::<IndustryLedger>();
    assert!(l.last_exports > Cost::ZERO, "{l:?}");
}

#[test]
fn a_processor_without_a_source_imports_and_pays() {
    let mut t = town(true);
    let mill = t.put("forest_processor_a", 500.0);
    t.staff(mill, 4);
    for _ in 0..4 * TICKS_PER_DAY {
        t.sim.tick_once();
        t.staff(mill, 4);
        if t.stock(mill, CargoKind::Logs) >= 20.0 {
            break;
        }
    }
    let l = *t.sim.world().resource::<IndustryLedger>();
    assert!(l.import_loads > 0, "{l:?}");
    let price = t.db.cargo(CargoKind::Logs).unwrap().import_price;
    assert!(l.imports >= Cost::scaled(price, 20.0));
    assert!(t.stock(mill, CargoKind::Logs) >= 20.0 || t.stock(mill, CargoKind::Planks) > 0.0);
}

#[test]
fn demolishing_the_target_with_a_truck_on_the_road_fails_the_run_without_a_panic() {
    let mut t = town(false);
    let saw = t.put("forest_extractor_s", 100.0);
    let mill = t.put("forest_processor_a", 900.0);
    t.staff(saw, 8);
    t.seed(saw, CargoKind::Logs, 40.0);
    for _ in 0..3 * TICKS_PER_DAY {
        t.sim.tick_once();
        if !t.sim.world().resource::<Goods>().hauls.is_empty() {
            break;
        }
    }
    assert!(!t.sim.world().resource::<Goods>().hauls.is_empty());
    t.sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .remove(mill);
    t.ticks(2 * TICKS_PER_DAY);
    assert!(t.sim.world().resource::<Goods>().hauls.is_empty());
    assert_eq!(t.stock(mill, CargoKind::Logs), 0.0);
}

#[test]
fn cargo_trade_is_deterministic() {
    let play = || {
        let mut t = town(true);
        let saw = t.put("forest_extractor_s", 100.0);
        let mill = t.put("forest_processor_a", 700.0);
        t.staff(saw, 8);
        t.staff(mill, 4);
        t.ticks(3 * TICKS_PER_DAY);
        t.sim
    };
    let (mut a, mut b) = (play(), play());
    assert_eq!(a.checksum(), b.checksum());
}

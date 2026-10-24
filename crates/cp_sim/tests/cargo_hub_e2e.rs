//! T-216: the cargo station. A station joined to the railway takes the exports and brings the
//! imports by cargo train, when it is nearer than the entry highway; one without a railway is not
//! used; a demolished one loses its stock without a panic; the stock survives a save; replays agree.

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
use cp_transit::CargoHubs;

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
    // Content prices: the market (§21) stays at its normal level.
    sim.world_mut().insert_resource(cp_content::MarketFrozen);
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

impl Town {
    /// A cargo station at `at` on the street, its platform a 160 m rail joined to an `Outside`
    /// node of a railway at z = 300 when `rail`.
    fn station(&mut self, at: f32, rail: bool) -> BuildingId {
        let id = self.put("cargo_station", at);
        let rail_def = self.db.road_id("rail_2t").unwrap();
        let def = self.db.road(rail_def).clone();
        let z = 300.0 + at;
        let platform = {
            let mut g = self.sim.world_mut().resource_mut::<NetGraph>();
            if rail {
                g.add_node(
                    Vec3::new(-160.0, 0.0, z),
                    NodeKind::Outside,
                    Elevation::Ground,
                );
            }
            let from = if rail { -160.0 } else { 0.0 };
            g.add_segment(
                &def,
                rail_def,
                CubicBezier3::line(Vec3::new(from, 0.0, z), Vec3::new(from + 160.0, 0.0, z)),
                Elevation::Ground,
            )
            .unwrap()
        };
        cp_transit::add_station(self.sim.world_mut(), id, platform).unwrap();
        self.sim.tick_once();
        id
    }

    fn hub_stock(&self, b: BuildingId) -> f32 {
        self.sim
            .world()
            .resource::<CargoHubs>()
            .hubs
            .get(&b)
            .map_or(0.0, |h| h.out_total())
    }

    fn ledger(&self) -> IndustryLedger {
        *self.sim.world().resource::<IndustryLedger>()
    }
}

#[test]
fn exports_go_to_a_station_joined_to_the_railway_and_are_paid_when_the_train_leaves() {
    let mut t = town(true);
    let saw = t.put("forest_extractor_s", 100.0);
    t.staff(saw, 8);
    t.seed(saw, CargoKind::Logs, 40.0);
    let station = t.station(130.0, true);
    // The truck drives the 30 m to the station, not the 100 m to the highway; the money comes with
    // the train, not with the truck.
    let mut at_station = false;
    for _ in 0..2 * TICKS_PER_DAY {
        t.sim.tick_once();
        if t.hub_stock(station) > 0.0 {
            at_station = true;
            break;
        }
    }
    assert!(at_station, "the export waits at the station");
    assert_eq!(t.ledger().export_loads, 0, "nothing sold before the train");
    let before = t.hub_stock(station);
    // T-320a: the train pays the day's price, 1.5 x at a level of 150 %.
    t.sim
        .world_mut()
        .resource_mut::<cp_content::Market>()
        .get_mut(CargoKind::Logs)
        .index = 150;
    // The train comes every cargo_train_interval_h (12 h).
    for _ in 0..TICKS_PER_DAY {
        t.sim.tick_once();
        if t.ledger().export_loads > 0 {
            break;
        }
    }
    let l = t.ledger();
    assert!(l.export_loads > 0, "{l:?}");
    let price = t.db.cargo(CargoKind::Logs).unwrap().export_price * 1.5;
    assert!(l.exports >= Cost::scaled(price, f64::from(before)), "{l:?}");
    assert!(t.hub_stock(station) < before);
}

#[test]
fn a_station_without_a_railway_is_not_used() {
    let mut t = town(true);
    let saw = t.put("forest_extractor_s", 100.0);
    t.staff(saw, 8);
    t.seed(saw, CargoKind::Logs, 40.0);
    let station = t.station(130.0, false);
    for _ in 0..4 * TICKS_PER_DAY {
        t.sim.tick_once();
        if t.ledger().export_loads > 0 {
            break;
        }
    }
    assert!(t.ledger().export_loads > 0, "it went out by the highway");
    assert_eq!(t.hub_stock(station), 0.0);
}

#[test]
fn imports_come_by_train_and_a_truck_takes_them_on_from_the_station() {
    let mut t = town(true);
    let mill = t.put("forest_processor_a", 500.0);
    t.staff(mill, 4);
    let station = t.station(480.0, true);
    let mut ordered = false;
    let mut got = false;
    for _ in 0..6 * TICKS_PER_DAY {
        t.sim.tick_once();
        t.staff(mill, 4);
        let hubs = t.sim.world().resource::<CargoHubs>();
        ordered |= hubs
            .hubs
            .get(&station)
            .is_some_and(|h| !h.orders.is_empty());
        if t.stock(mill, CargoKind::Logs) >= 20.0 || t.stock(mill, CargoKind::Planks) > 0.0 {
            got = true;
            break;
        }
    }
    assert!(ordered, "the import was ordered at the station");
    assert!(
        got,
        "and arrived: {:?}",
        t.sim.world().resource::<CargoHubs>()
    );
    assert!(t.ledger().import_loads > 0);
    // The train rides out again and the visit is counted.
    t.ticks(TICKS_PER_DAY / 2);
    assert!(t.sim.world().resource::<CargoHubs>().hubs[&station].trains > 0);
}

#[test]
fn demolishing_a_station_with_stock_loses_the_stock_without_a_panic() {
    let mut t = town(true);
    let saw = t.put("forest_extractor_s", 100.0);
    t.staff(saw, 8);
    t.seed(saw, CargoKind::Logs, 40.0);
    let station = t.station(130.0, true);
    for _ in 0..2 * TICKS_PER_DAY {
        t.sim.tick_once();
        if t.hub_stock(station) > 0.0 {
            break;
        }
    }
    assert!(t.hub_stock(station) > 0.0);
    t.sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .remove(station);
    t.ticks(TICKS_PER_DAY);
    assert!(t.sim.world().resource::<CargoHubs>().is_empty());
}

#[test]
fn cargo_hubs_are_deterministic() {
    let run = || {
        let mut t = town(true);
        let saw = t.put("forest_extractor_s", 100.0);
        t.staff(saw, 8);
        t.seed(saw, CargoKind::Logs, 60.0);
        t.station(130.0, true);
        t.ticks(3 * TICKS_PER_DAY);
        t.sim.checksum()
    };
    assert_eq!(run(), run());
}

#[test]
fn the_cargo_train_is_a_vehicle_with_a_pose_that_rides_in_stands_and_rides_out() {
    let mut t = town(true);
    let saw = t.put("forest_extractor_s", 100.0);
    t.staff(saw, 8);
    t.seed(saw, CargoKind::Logs, 40.0);
    let station = t.station(130.0, true);
    let (mut inbound, mut dwell, mut outbound) = (false, false, false);
    let mut paid_at_dwell = false;
    let mut moved = false;
    let mut last_s = None;
    for _ in 0..3 * TICKS_PER_DAY {
        t.sim.tick_once();
        let view = t.sim.view();
        let Some(train) = view.cargo_trains.first() else {
            if outbound {
                break;
            }
            continue;
        };
        assert_eq!(train.station, station);
        match train.phase {
            cp_transit::TrainPhase::Inbound => {
                inbound = true;
                if let Some(s) = last_s {
                    moved |= train.pose.s > s;
                }
                last_s = Some(train.pose.s);
            }
            cp_transit::TrainPhase::Dwell { .. } => {
                dwell = true;
                paid_at_dwell |= t.ledger().export_loads > 0;
                assert!(
                    (train.pose.s - train.pose.length()).abs() < 1.0,
                    "at the platform"
                );
            }
            cp_transit::TrainPhase::Outbound => outbound = true,
        }
    }
    assert!(inbound && dwell && outbound, "{inbound} {dwell} {outbound}");
    assert!(moved, "it moved along the track while inbound");
    assert!(!paid_at_dwell, "paid when it leaves, not when it stands");
    assert!(t.ledger().export_loads > 0);
    assert!(t.sim.view().cargo_trains.is_empty());
}

#[test]
fn the_unique_factory_premium_follows_its_goods_through_a_hub() {
    use cp_content::CargoKind::Goods;
    let mul = content().balance().factory_export_mul;
    let mut hubs = CargoHubs::default();
    let st = BuildingId::new(1, 0);
    hubs.deposit(st, Goods, 100.0, 100.0 * (mul - 1.0), Tick(0), 10);
    hubs.deposit(st, Goods, 100.0, 0.0, Tick(0), 10);
    let h = &hubs.hubs[&st];
    assert_eq!(h.out_total(), 200.0);
    assert!((h.premium - 100.0 * (mul - 1.0)).abs() < 1e-3);
}

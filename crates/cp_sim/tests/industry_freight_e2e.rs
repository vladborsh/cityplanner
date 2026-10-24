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

/// The shipped content with T-306's trade shares at 0: these tests are about the rules of
/// T-210 (trade only when nobody near can take or give).
fn content() -> Arc<ContentDb> {
    content_sharing(0.0, 0.0)
}

/// The shipped content with `industry_export_share` and `industry_import_share` set.
fn content_sharing(export: f32, import: f32) -> Arc<ContentDb> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content");
    let db = ContentDb::load_dir_with_balance(root, |text| {
        text.lines()
            .map(|l| {
                let t = l.trim_start();
                if t.starts_with("industry_export_share:") {
                    format!("    industry_export_share: {export:?},")
                } else if t.starts_with("industry_import_share:") {
                    format!("    industry_import_share: {import:?},")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
    .unwrap();
    assert_eq!(db.balance().industry_export_share, export);
    assert_eq!(db.balance().industry_import_share, import);
    Arc::new(db)
}

struct Town {
    sim: SimRunner,
    db: Arc<ContentDb>,
    seg: cp_net::SegmentId,
}

/// A 1 km street along z = 5 (an `Outside` node at x = 0 when `outside`), a forestry area over
/// it with its Main building at x = 40, and forest under the whole street.
fn town(outside: bool) -> Town {
    town_on(content(), outside)
}

/// [`town`] on the content given.
fn town_on(db: Arc<ContentDb>, outside: bool) -> Town {
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

/// T-306: traffic grows with industry. With the trade shares at 1 a sawmill's logs go out by the
/// entry highway although the processor stands 400 m away, and the processor orders its logs
/// from the outside although the sawmill stands near; at 0 (the old rule) the sawmill's first
/// load goes to the processor and it imports nothing (later loads may still go out once the
/// processor's store is full, as before).
#[test]
fn the_trade_shares_send_loads_abroad_and_order_them_in_past_a_neighbour() {
    let run = |share: f32| {
        let mut t = town_on(content_sharing(share, share), true);
        let saw = t.put("forest_extractor_s", 100.0);
        let mill = t.put("forest_processor_a", 500.0);
        t.staff(saw, 8);
        t.staff(mill, 4);
        t.seed(saw, CargoKind::Logs, 40.0);
        let mut first_to: Option<Endpoint> = None;
        for _ in 0..4 * TICKS_PER_DAY {
            t.sim.tick_once();
            t.staff(saw, 8);
            t.staff(mill, 4);
            if first_to.is_none() {
                let g = t.sim.world().resource::<Goods>();
                first_to = g
                    .hauls
                    .values()
                    .find(|h| h.from == Endpoint::Building(saw))
                    .map(|h| h.to);
            }
        }
        (
            first_to.expect("the sawmill sent a load"),
            *t.sim.world().resource::<IndustryLedger>(),
            mill,
        )
    };
    let (to, all, _) = run(1.0);
    assert!(matches!(to, Endpoint::Outside(_)), "past the mill: {to:?}");
    assert!(all.import_loads > 0, "imports past the sawmill: {all:?}");
    let (to, none, mill) = run(0.0);
    assert_eq!(to, Endpoint::Building(mill), "the old rule: to the mill");
    assert_eq!(none.import_loads, 0, "the old rule: {none:?}");
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

// ---- Warehouses (T-214) ----

fn set_warehouse(
    t: &mut Town,
    b: BuildingId,
    cargo: Option<CargoKind>,
    mode: cp_industry::WarehouseMode,
) -> Result<(), String> {
    use bevy_ecs::message::Messages;
    use cp_core::{CommandError, CommandOutcome};
    let cmd = cp_sim::SimCommand::Industry(cp_industry::IndustryCommand::SetWarehouse {
        building: b,
        cargo,
        mode,
    });
    t.sim.push_command(cmd, t.sim.tick());
    t.sim.apply_due_commands();
    let out: Vec<CommandOutcome> = t
        .sim
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

use cp_industry::WarehouseMode::{Balanced, Empty, Fill};

#[test]
fn a_sawmill_without_a_processor_fills_a_balanced_warehouse_instead_of_exporting() {
    let mut t = town(true);
    let saw = t.put("forest_extractor_s", 100.0);
    let store = t.put("forest_warehouse", 600.0);
    t.staff(saw, 8);
    t.seed(saw, CargoKind::Logs, 40.0);
    for _ in 0..4 * TICKS_PER_DAY {
        t.sim.tick_once();
        if t.stock(store, CargoKind::Logs) >= 20.0 {
            break;
        }
    }
    assert!(t.stock(store, CargoKind::Logs) >= 20.0);
    assert_eq!(t.sim.world().resource::<IndustryLedger>().export_loads, 0);
}

#[test]
fn a_processor_takes_from_a_warehouse_instead_of_importing() {
    let mut t = town(true);
    let mill = t.put("forest_processor_a", 500.0);
    let store = t.put("forest_warehouse", 800.0);
    t.staff(mill, 4);
    t.seed(store, CargoKind::Logs, 900.0); // over half of 1500
    for _ in 0..4 * TICKS_PER_DAY {
        t.sim.tick_once();
        t.staff(mill, 4);
        if t.stock(mill, CargoKind::Logs) >= 20.0 || t.stock(mill, CargoKind::Planks) > 0.0 {
            break;
        }
    }
    let l = *t.sim.world().resource::<IndustryLedger>();
    assert_eq!(l.import_loads, 0, "{l:?}");
    assert!(t.stock(mill, CargoKind::Logs) >= 20.0 || t.stock(mill, CargoKind::Planks) > 0.0);
    assert!(t.stock(store, CargoKind::Logs) < 900.0);
}

#[test]
fn a_filling_warehouse_gives_nothing_and_an_emptying_one_exports() {
    // Fill: the processor has to import.
    let mut t = town(true);
    let mill = t.put("forest_processor_a", 500.0);
    let store = t.put("forest_warehouse", 800.0);
    t.staff(mill, 4);
    t.seed(store, CargoKind::Logs, 900.0);
    set_warehouse(&mut t, store, None, Fill).unwrap();
    for _ in 0..3 * TICKS_PER_DAY {
        t.sim.tick_once();
        t.staff(mill, 4);
    }
    assert!(t.sim.world().resource::<IndustryLedger>().import_loads > 0);
    assert_eq!(t.stock(store, CargoKind::Logs), 900.0);

    // Empty: nobody needs it, so it goes out of the map.
    let mut t = town(true);
    let store = t.put("forest_warehouse", 800.0);
    t.seed(store, CargoKind::Logs, 60.0);
    set_warehouse(&mut t, store, Some(CargoKind::Logs), Empty).unwrap();
    for _ in 0..4 * TICKS_PER_DAY {
        t.sim.tick_once();
        if t.sim.world().resource::<IndustryLedger>().export_loads > 0 {
            break;
        }
    }
    assert!(t.sim.world().resource::<IndustryLedger>().export_loads > 0);
    assert!(t.stock(store, CargoKind::Logs) < 60.0);
    // And a Balanced warehouse under half never exports.
    let mut t = town(true);
    let store = t.put("forest_warehouse", 800.0);
    t.seed(store, CargoKind::Logs, 60.0);
    set_warehouse(&mut t, store, None, Balanced).unwrap();
    t.ticks(3 * TICKS_PER_DAY);
    assert_eq!(t.sim.world().resource::<IndustryLedger>().export_loads, 0);
    assert_eq!(t.stock(store, CargoKind::Logs), 60.0);
}

#[test]
fn set_warehouse_checks_the_building_and_the_cargo() {
    let mut t = town(false);
    let store = t.put("forest_warehouse", 800.0);
    let saw = t.put("forest_extractor_s", 100.0);
    let e = set_warehouse(&mut t, store, Some(CargoKind::Ore), Fill).unwrap_err();
    assert!(e.contains("not a cargo of this warehouse's chain"), "{e}");
    let e = set_warehouse(&mut t, saw, None, Fill).unwrap_err();
    assert!(e.contains("not a warehouse"), "{e}");
    let e = set_warehouse(&mut t, Id::new(999, 0), None, Fill).unwrap_err();
    assert!(e.contains("no such building"), "{e}");
    set_warehouse(&mut t, store, Some(CargoKind::Planks), Fill).unwrap();
    let s = t.sim.view().warehouses.get(store);
    assert_eq!((s.cargo, s.mode), (Some(CargoKind::Planks), Fill));
    // Back to the default: the entry goes away.
    set_warehouse(&mut t, store, None, Balanced).unwrap();
    assert!(t.sim.view().warehouses.list.is_empty());
}

#[test]
fn a_logs_haul_rides_the_log_truck_not_the_goods_truck() {
    use cp_content::VehicleKind;
    use cp_traffic::{Agents, ServiceRuns};
    let mut t = town(false);
    let saw = t.put("forest_extractor_s", 100.0);
    let _mill = t.put("forest_processor_a", 700.0);
    t.staff(saw, 8);
    t.seed(saw, CargoKind::Logs, 60.0);
    // The defs of the four chain trucks carry their kinds.
    for (id, kind) in [
        ("truck_farm", VehicleKind::CargoFarm),
        ("truck_logs", VehicleKind::CargoLogs),
        ("truck_ore", VehicleKind::CargoOre),
        ("truck_tanker", VehicleKind::CargoTanker),
    ] {
        let (_, d) = t.db.vehicles().find(|(_, d)| d.id == id).expect(id);
        assert_eq!(d.kind, kind, "{id}");
        assert!(d.length > 0.0);
    }
    let (run_kind, vehicle_kind) = 'found: {
        for _ in 0..3 * TICKS_PER_DAY {
            t.sim.tick_once();
            let w = t.sim.world();
            if let Some((_, r)) = w.resource::<ServiceRuns>().iter().next()
                && let Some((_, v)) = w.resource::<Agents>().vehicles().next()
            {
                break 'found (r.kind, v.kind_id());
            }
        }
        panic!("no log truck appeared");
    };
    assert_eq!(run_kind, VehicleKind::CargoLogs as u8);
    assert_eq!(vehicle_kind, VehicleKind::CargoLogs as u8);
}

// ---- Unique factories (T-215) ----

impl Town {
    /// Raises the level of the area.
    fn area_level(&mut self, level: u8) {
        self.sim
            .world_mut()
            .resource_mut::<Areas>()
            .get_mut(AreaId(1))
            .unwrap()
            .level = level;
    }
}

/// A shop def to deliver to: the first plain commercial one.
fn shop_name(db: &ContentDb) -> String {
    db.buildings()
        .find(|(_, d)| {
            d.zone == cp_content::ZoneType::ComLow
                && d.service.is_none()
                && d.industry.is_none()
                && d.spec.is_none()
        })
        .map(|(_, d)| d.id.clone())
        .unwrap()
}

fn factory_town(outside: bool) -> (Town, BuildingId) {
    let mut t = town(outside);
    t.area_level(2);
    let f = t.put("food_factory", 300.0);
    t.staff(f, 10);
    t.sim
        .world_mut()
        .resource_mut::<IndustryState>()
        .stock
        .insert(
            f,
            vec![
                (CargoKind::AnimalProducts, 200.0),
                (CargoKind::Flour, 200.0),
            ],
        );
    (t, f)
}

#[test]
fn a_factory_with_both_inputs_makes_goods_and_without_one_says_no_input() {
    let (mut t, f) = factory_town(false);
    t.ticks(TICKS_PER_DAY);
    assert!(
        t.sim.world().resource::<Goods>().stock_of(f) > 0.0
            || t.sim
                .world()
                .resource::<Goods>()
                .hauls
                .values()
                .any(|h| h.from == Endpoint::Building(f))
    );
    assert!(t.stock(f, CargoKind::Flour) < 200.0 && t.stock(f, CargoKind::AnimalProducts) < 200.0);
    assert_eq!(
        t.sim.world().resource::<IndustryState>().problem_of(f),
        cp_industry::IndustryProblem::None
    );
    // Flour runs out: nothing is made and it says why.
    t.sim
        .world_mut()
        .resource_mut::<IndustryState>()
        .stock
        .insert(f, vec![(CargoKind::AnimalProducts, 200.0)]);
    t.staff(f, 10);
    t.ticks(TICKS_PER_DAY);
    assert_eq!(
        t.sim.world().resource::<IndustryState>().problem_of(f),
        cp_industry::IndustryProblem::NoInput
    );
}

#[test]
fn the_goods_of_a_factory_reach_a_shop() {
    let (mut t, f) = factory_town(false);
    let name = shop_name(&t.db);
    let shop = t.put(&name, 800.0);
    for _ in 0..4 * TICKS_PER_DAY {
        t.sim.tick_once();
        t.staff(f, 10);
        if t.sim.world().resource::<Goods>().stock_of(shop) > 0.0 {
            break;
        }
    }
    assert!(t.sim.world().resource::<Goods>().stock_of(shop) > 0.0);
}

#[test]
fn without_shops_the_factorys_goods_are_exported_at_four_times_the_price() {
    let (mut t, f) = factory_town(true);
    for _ in 0..4 * TICKS_PER_DAY {
        t.sim.tick_once();
        t.staff(f, 10);
        if t.sim
            .world()
            .resource::<cp_transit::TransitLedger>()
            .export_loads
            > 0
        {
            break;
        }
    }
    let l = *t.sim.world().resource::<cp_transit::TransitLedger>();
    assert!(l.export_loads > 0, "{l:?}");
    let bal = t.db.balance();
    assert_eq!(
        l.exports,
        Cost::scaled(
            bal.export_price * bal.factory_export_mul,
            20.0 * f64::from(l.export_loads)
        )
    );
}

#[test]
fn factories_are_deterministic() {
    let play = || {
        let (mut t, f) = factory_town(true);
        for _ in 0..3 * TICKS_PER_DAY {
            t.sim.tick_once();
            t.staff(f, 10);
        }
        t.sim
    };
    let (mut a, mut b) = (play(), play());
    assert_eq!(a.checksum(), b.checksum());
}

impl Town {
    /// Fills every job slot of the building, each education level.
    fn staff_full(&mut self, b: BuildingId) {
        let jobs = self
            .db
            .building(
                self.sim
                    .world()
                    .resource::<BuildingGrid>()
                    .get(b)
                    .unwrap()
                    .def,
            )
            .jobs;
        let mut g = self.sim.world_mut().resource_mut::<CitizenGrid>();
        for (edu, &n) in jobs.iter().enumerate() {
            let mut left = usize::from(n).saturating_sub(usize::from(g.jobs_filled(b, edu)));
            while left > 0 {
                let k = left.min(4);
                left -= k;
                let hid = g.add_household(b, 0, Tick(0), &vec![(9000, edu as u8, false); k]);
                let members: Vec<_> = g
                    .household(hid)
                    .unwrap()
                    .members
                    .iter()
                    .flatten()
                    .copied()
                    .collect();
                for m in members {
                    g.set_job(m, b, edu as u8);
                }
            }
        }
    }
}

/// T-316: the money of one setup over `days` days, as the treasury's change, and the running
/// trade balance (exports minus imports) at the end of each day.
fn run_setup(buildings: &[(&str, f32)], days: u64) -> (i64, Vec<i64>) {
    let mut t = town(true);
    let ids: Vec<_> = buildings.iter().map(|&(n, at)| t.put(n, at)).collect();
    for &id in &ids {
        t.staff_full(id);
    }
    let start = t.sim.world().resource::<cp_core::Treasury>().balance;
    let (mut prev_e, mut prev_i, mut trade, mut daily) = (Cost::ZERO, Cost::ZERO, 0i64, vec![]);
    for _ in 0..days {
        for _ in 0..TICKS_PER_DAY {
            t.sim.tick_once();
            let l = *t.sim.world().resource::<IndustryLedger>();
            // The week's settlement resets the sums: a smaller one is a new week.
            trade += (if l.exports >= prev_e {
                l.exports - prev_e
            } else {
                l.exports
            })
            .0;
            trade -= (if l.imports >= prev_i {
                l.imports - prev_i
            } else {
                l.imports
            })
            .0;
            (prev_e, prev_i) = (l.exports, l.imports);
        }
        for &id in &ids {
            t.staff_full(id);
        }
        daily.push(trade);
    }
    let end = t.sim.world().resource::<cp_core::Treasury>().balance;
    ((end - start).0, daily)
}

/// T-316: a processor fed only by imports loses money at first (the import is bought before
/// the output sells), then earns; per day, a full local chain beats it, and it beats a generic
/// industrial building.
#[test]
fn a_processor_on_imports_pays_back_between_a_full_chain_and_plain_industrial_zoning() {
    let days = 14;
    let (control, _) = run_setup(&[], days);
    let (chain, _) = run_setup(
        &[("forest_extractor_l", 100.0), ("forest_processor_a", 500.0)],
        days,
    );
    let (imports, trade) = run_setup(&[("forest_processor_a", 500.0)], days);
    let (generic, _) = run_setup(&[("ind_1", 500.0)], days);
    let (furn, _) = run_setup(&[("furniture_factory", 500.0)], days);
    let (food, _) = run_setup(&[("food_factory", 500.0)], days);
    let (car, _) = run_setup(&[("car_factory", 500.0)], days);
    let net = |x: i64| (x - control) as f64 / 100.0 / days as f64;
    eprintln!(
        "per day: chain {:.1}, imports {:.1}, generic {:.1}",
        net(chain),
        net(imports),
        net(generic)
    );
    // Factories on imported products pay too, below a full chain's day.
    for (name, v) in [("furniture", furn), ("food", food), ("car", car)] {
        eprintln!("factory on imports {name}: {:.1}", net(v));
        assert!(net(v) > 0.0, "{name} factory on imports: {}", net(v));
    }
    assert!(net(imports) > 0.0, "positive: {}", net(imports));
    assert!(
        net(chain) > net(imports),
        "chain {} > imports {}",
        net(chain),
        net(imports)
    );
    assert!(
        net(imports) > net(generic),
        "imports {} > generic {}",
        net(imports),
        net(generic)
    );
    // The trade balance dips (imports paid first) and ends above zero.
    let low = *trade.iter().min().unwrap();
    assert!(low < 0, "a dip: {trade:?}");
    assert!(*trade.last().unwrap() > 0, "ends positive: {trade:?}");
}

/// T-316: the area's book holds what its buildings made and the trade they did, and the weekly
/// settlement adds the taxes and the upkeep of its chain buildings.
#[test]
fn the_area_book_holds_the_weeks_production_trade_taxes_and_upkeep() {
    let mut t = town(true);
    let mill = t.put("forest_processor_a", 500.0);
    t.staff_full(mill);
    for _ in 0..9 * TICKS_PER_DAY {
        t.sim.tick_once();
        t.staff_full(mill);
    }
    let books = t.sim.world().resource::<cp_industry::AreaBooks>().clone();
    let last = books.last_of(1).expect("a settled week for area 1");
    assert!(last.imports > Cost::ZERO, "{last:?}");
    assert!(last.exports > Cost::ZERO, "{last:?}");
    assert!(
        last.produced
            .get(&CargoKind::Planks)
            .copied()
            .unwrap_or(0.0)
            > 0.0,
        "{last:?}"
    );
    let (taxes, upkeep) = books.money[&1];
    assert!(
        taxes > Cost::ZERO && upkeep > Cost::ZERO,
        "{taxes:?} {upkeep:?}"
    );
    // The book adds up with the city-wide ledger (one area).
    let l = *t.sim.world().resource::<IndustryLedger>();
    assert_eq!(last.imports, l.last_imports);
    assert_eq!(last.exports, l.last_exports);
}

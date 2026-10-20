//! BUG-095: shops out of goods in a small town. The streets and zones of `town:N` on a new
//! game's map, joined to the entry highway, grown from nothing with the game's own demand
//! (decay off, so no utilities are needed); per day the goods chain and the demand.
//!
//! The cause: an import truck started at the `Outside` node nearest the shop and an export
//! truck ended at the one nearest the factory, but the entry highway is two one-way
//! carriageways — the inbound one starts at one node, the outbound one ends at the other. The
//! shops nearer the outbound end got trucks that could not drive in: the run failed, the load was
//! lost, the next day the same (21 of the 52 shops of an industry-less town stood out of goods
//! from its third day on). `cp_net::outside_ways` now picks by the way a vehicle can use a node.
//!
//! `cargo nextest run -p cp_app --test goods_e2e --run-ignored all --no-capture` prints the
//! probe (`PROBE_IND=0.3`: the share of the industrial block zoned; `PROBE_NO_ROAD=1`: not joined
//! to the highway, nothing to import).

use std::path::Path;

use cp_building::{BuildingFlags, BuildingGrid, BuildingState, Demand};
use cp_citizen::CitizenGrid;
use cp_content::{SimContent, ZoneType};
use cp_core::SimLimits;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{ElevationMode, NetCommand, RoadPath};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;
use cp_transit::{Endpoint, Goods, TransitLedger};
use cp_zone::ZoneGrid;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// The town with `keep` of its industrial block zoned (from its west end), joined to the entry
/// highway (its stub ends at (5120, 4348), the town's top street has a junction at (5120, 5000))
/// or not.
fn town(keep: f32, joined: bool) -> SimRunner {
    let t = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
    let mut sim = cp_app::build_sim(t, SimLimits::desktop(), 10.0, false, None);
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance = cp_core::Cost(1_000_000_000);
    let _ = cp_app::town::lay_out_town(&mut sim);
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
    {
        let zones = sim.world().resource::<ZoneGrid>();
        let ind: Vec<_> = zones
            .blocks()
            .flat_map(|(id, _)| zones.block_cells(id))
            .filter(|c| zones.zone_of(*c) == Some(ZoneType::Industrial))
            .filter_map(|c| Some((c, zones.cell_footprint(c)?.c.x)))
            .collect();
        let (lo, hi) = ind
            .iter()
            .fold((f32::MAX, f32::MIN), |a, (_, x)| (a.0.min(*x), a.1.max(*x)));
        let cut = lo + (hi - lo) * keep;
        let drop: Vec<_> = ind
            .into_iter()
            .filter(|(_, x)| *x > cut + 0.1)
            .map(|(c, _)| c)
            .collect();
        let mut zones = sim.world_mut().resource_mut::<ZoneGrid>();
        for c in drop {
            zones.set_zone(c, ZoneType::None);
        }
    }
    if joined {
        let db = sim.world().resource::<SimContent>().0.clone();
        sim.push_command(
            SimCommand::Net(NetCommand::BuildRoad {
                road: db.road_id("basic_2l").unwrap(),
                path: RoadPath::Straight {
                    from: bevy_math::Vec2::new(5120.0, 4348.0),
                    to: bevy_math::Vec2::new(5120.0, 5000.0),
                },
                elevation: ElevationMode::Ground,
            }),
            sim.tick(),
        );
        sim.tick_once();
    }
    sim
}

#[derive(Default, Debug)]
struct Day {
    residents: usize,
    shops: usize,
    no_goods: usize,
    shop_jobs: u32,
    shop_stock: f32,
    goods_ind: usize,
    ind_jobs: u32,
    factory_stock: f32,
    imports: usize,
    exports: usize,
}

fn day(sim: &SimRunner) -> Day {
    let w = sim.world();
    let db = &w.resource::<SimContent>().0;
    let b = w.resource::<BuildingGrid>();
    let c = w.resource::<CitizenGrid>();
    let g = w.resource::<Goods>();
    let mut d = Day {
        residents: c.citizens().count(),
        ..Default::default()
    };
    for (id, bl) in b.iter().filter(|(_, b)| b.state == BuildingState::Active) {
        let def = db.building(bl.def);
        let jobs: u32 = def.jobs.iter().map(|j| u32::from(*j)).sum();
        match def.zone {
            ZoneType::ComLow | ZoneType::ComHigh if def.industry.is_none() => {
                d.shops += 1;
                d.shop_jobs += jobs;
                d.shop_stock += g.stock_of(id);
                if bl.flags.0 & BuildingFlags::NO_GOODS != 0 {
                    d.no_goods += 1;
                }
            }
            ZoneType::Industrial
                if def.service.is_none() && def.unique.is_none() && def.park.is_none() =>
            {
                d.goods_ind += 1;
                d.ind_jobs += jobs;
                d.factory_stock += g.stock_of(id);
            }
            _ => {}
        }
    }
    for h in g.hauls.values() {
        match (h.from, h.to) {
            (Endpoint::Outside(_), _) => d.imports += 1,
            (_, Endpoint::Outside(_)) => d.exports += 1,
            _ => {}
        }
    }
    d
}

fn run_day(sim: &mut SimRunner) {
    for _ in 0..TICKS_PER_DAY {
        sim.tick_once();
    }
}

/// A town of ~400-700 with its industrial block, and one without industry, over 7 days: fewer
/// than 5 % of the shops are ever out of goods (the industry-less one imports).
#[test]
fn a_small_town_keeps_its_shops_in_goods() {
    for keep in [1.0, 0.0] {
        let mut sim = town(keep, true);
        let mut imported = 0;
        for k in 1..=7 {
            run_day(&mut sim);
            let d = day(&sim);
            imported += sim.world().resource::<TransitLedger>().import_loads;
            assert!(d.residents >= 300 && d.shops >= 40, "day {k}: {d:?}");
            assert!(
                d.no_goods * 100 < 5 * d.shops,
                "industry {keep}, day {k}: {} of {} shops out of goods",
                d.no_goods,
                d.shops
            );
        }
        if keep == 0.0 {
            assert!(imported > 0, "an industry-less town imports");
        }
    }
}

#[test]
#[ignore = "BUG-095 probe: prints numbers"]
fn goods_chain_of_a_500_town() {
    let keep = std::env::var("PROBE_IND")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    let mut sim = town(keep, std::env::var("PROBE_NO_ROAD").is_err());
    for k in 0..20 {
        let d = day(&sim);
        let dem = *sim.world().resource::<Demand>();
        let led = sim.world().resource::<TransitLedger>();
        let db = sim.world().resource::<SimContent>().0.clone();
        let short =
            cp_economy::shortage_jobs(sim.world().resource::<BuildingGrid>(), &db, db.balance());
        eprintln!(
            "day {k}: res {} | shops {} no_goods {} jobs {} stock {:.0} | goods-industry {} jobs {} stock {:.0} | in flight: imports {} exports {} | loads imported {} exported {} | demand R {:.1} C {:.1} I {:.1} (shortage term {short:.1})",
            d.residents,
            d.shops,
            d.no_goods,
            d.shop_jobs,
            d.shop_stock,
            d.goods_ind,
            d.ind_jobs,
            d.factory_stock,
            d.imports,
            d.exports,
            led.import_loads,
            led.export_loads,
            dem.r,
            dem.c,
            dem.i
        );
        run_day(&mut sim);
    }
}

/// The entry highway of a new game: the inbound carriageway starts at one `Outside` node (where
/// trucks from the outside start), the outbound one ends at the other (where they leave).
#[test]
fn the_entry_highway_has_one_way_in_and_one_way_out() {
    let t = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
    let sim = cp_app::build_sim(t, SimLimits::desktop(), 10.0, false, None);
    let w = sim.world();
    let net = w.resource::<cp_net::NetGraph>();
    let db = &w.resource::<SimContent>().0;
    let (entries, exits) = cp_net::outside_ways(net, db);
    assert_eq!(
        (entries.len(), exits.len()),
        (1, 1),
        "{entries:?} {exits:?}"
    );
    assert_ne!(entries[0].0, exits[0].0, "two carriageways, two ends");
    for (id, _) in &entries {
        let n = net.node(*id).unwrap();
        assert!(
            n.segments
                .iter()
                .any(|s| net.segment(*s).unwrap().start == *id)
        );
    }
}

/// The industrial demand asks for the jobs that would make the goods of the shops out of goods.
#[test]
fn shops_out_of_goods_raise_the_industrial_demand() {
    let mut sim = town(0.0, true);
    run_day(&mut sim);
    let db = sim.world().resource::<SimContent>().0.clone();
    let b = db.balance();
    assert_eq!(
        cp_economy::shortage_jobs(sim.world().resource::<BuildingGrid>(), &db, b),
        0.0,
        "all shops stocked"
    );
    let shops: Vec<_> = {
        let grid = sim.world().resource::<BuildingGrid>();
        grid.iter()
            .filter(|(_, bl)| {
                bl.state == BuildingState::Active
                    && matches!(
                        db.building(bl.def).zone,
                        ZoneType::ComLow | ZoneType::ComHigh
                    )
            })
            .map(|(id, bl)| {
                (
                    id,
                    db.building(bl.def)
                        .jobs
                        .iter()
                        .map(|j| u32::from(*j))
                        .sum::<u32>(),
                )
            })
            .take(3)
            .collect()
    };
    for (id, _) in &shops {
        sim.world_mut()
            .resource_mut::<BuildingGrid>()
            .set_flag(*id, BuildingFlags::NO_GOODS, true);
    }
    let slots: u32 = shops.iter().map(|(_, j)| j).sum();
    let want = slots as f32 * b.goods_per_job / b.goods_per_worker;
    let got = cp_economy::shortage_jobs(sim.world().resource::<BuildingGrid>(), &db, b);
    assert!(want > 0.0 && (got - want).abs() < 1e-3, "{got} vs {want}");
}

/// A service stands on an `Industrial` def: it makes no shop goods and its jobs are no industry
/// in the demand (they used to count against I like a factory's).
#[test]
fn a_service_makes_no_goods_and_is_no_industry_in_the_demand() {
    let mut sim = town(1.0, true);
    run_day(&mut sim);
    let db = sim.world().resource::<SimContent>().0.clone();
    let (seg, at) = {
        let net = sim.world().resource::<cp_net::NetGraph>();
        let (id, s) = net
            .segments()
            .find(|(_, s)| s.elevation == cp_net::Elevation::Ground)
            .unwrap();
        (id, bevy_math::Vec2::new(s.curve.p0.x, s.curve.p0.z))
    };
    let police = sim
        .world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(cp_building::Building {
            def: db.building_id("police_station").unwrap(),
            footprint: cp_building::Footprint::Free { w: 2, d: 2 },
            level: 1,
            state: BuildingState::Active,
            ready_at: cp_core::Tick(0),
            pos: at + bevy_math::Vec2::new(0.0, 30.0),
            rot: 0.0,
            access: Some(cp_building::AccessPoint {
                segment: seg,
                t: 0.5,
            }),
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
    run_day(&mut sim);
    run_day(&mut sim);
    let goods = sim.world().resource::<Goods>();
    assert_eq!(
        goods.stock_of(police),
        0.0,
        "a police station makes no goods"
    );
    assert!(
        goods
            .hauls
            .values()
            .all(|h| h.from != Endpoint::Building(police))
    );
    // A whole demand cycle: the industrial jobs are the grown industry's only.
    while !sim.tick().0.is_multiple_of(cp_building::DEMAND_PERIOD) {
        sim.tick_once();
    }
    let acc = *sim.world().resource::<cp_economy::DemandAcc>();
    let grown: u32 = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| {
            matches!(
                b.state,
                BuildingState::Active | BuildingState::UnderConstruction
            )
        })
        .map(|(_, b)| db.building(b.def))
        .filter(|d| d.zone == ZoneType::Industrial && cp_building::is_growable(d))
        .map(|d| d.jobs.iter().map(|j| u32::from(*j)).sum::<u32>())
        .sum();
    assert!(grown > 0);
    assert_eq!(
        acc.industrial_jobs, grown,
        "the police station's jobs are not industry"
    );
}

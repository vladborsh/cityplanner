//! Goods (§16 M3 D, T-169): factories make them, shops sell them, trucks carry them between.
//!
//! Once a day, in 64 buckets by `BuildingId.index % 64` (one bucket every 64 ticks), an `Active`
//! industrial building adds `goods_per_worker × filled jobs` to its stock (up to
//! `goods_stock_cap`) and a commercial one takes `goods_per_job × filled jobs` off. A factory
//! holding a `goods_truck_load` sends a truck (a `Cargo` run of `cp_traffic`) to the shop with the
//! least stock within `freight_radius_m` (straight-line distance; a tie goes to the smaller
//! `BuildingId`); a factory holding a quarter of a load sends the part load to an empty shop
//! (BUG-058). The load leaves the factory at once and is added to the shop when the truck is
//! there (with the vehicle pool full the run arrives abstractly). A shop that has stood empty for
//! `goods_empty_days` gets `NO_GOODS`; a delivery clears it. A run that fails (the shop was
//! demolished on the way, no route) loses its load.
//!
//! **Exports and imports (T-170).** A factory holding a load with no shop within the radius sends
//! the truck to the nearest `Outside` node of a highway instead: the load leaves the map and
//! `export_price × load` goes to the [`TransitLedger`] (and on to the week report) when the truck
//! is there. A shop below `goods_import_threshold` with no factory within the radius that holds
//! at least a quarter of a load (BUG-058; a weak factory does not block imports) and no truck on
//! its way gets a truck from the nearest `Outside` node, free for the treasury (it is traffic on the entry road). With
//! no `Outside` node nothing is exported or imported, as before.
//!
//! **Traffic grows with industry (T-306).** Whatever shop stands near, `industry_export_share` of a
//! factory's loads (part loads too) go out by the entry highway, and a working factory gets
//! `industry_import_share` of its days a truck of raw materials from the outside, used up on
//! arrival and counted as an import load; a hash of the factory, the day's bucket and the cargo
//! ([`trade_roll`]) picks them, so more industry is more trucks through the entry both ways.

use std::collections::{BTreeMap, BTreeSet};

use crate::fleet::TransitLedger;
use crate::hubs::{exit_for, hub_sites, is_hub_def};
use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_math::Vec2;
use cp_building::{BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{CargoKind, IndustryRole, SimContent, VehicleKind, ZoneType};
use cp_core::time::{TICKS_PER_DAY, TICKS_PER_HOUR};
use cp_core::{Cost, Tick};
use cp_net::{NetGraph, NodeId};
use cp_traffic::{Agents, RunId, RunPoint, RunState, ServiceRuns};
use serde::{Deserialize, Serialize};

/// A factory may send a part load of at least this share of `goods_truck_load` to an empty shop.
const PARTIAL_LOAD_SHARE: f32 = 0.25;

/// Which way a load would cross the map's edge (T-306).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// An export: out by the entry highway.
    Out,
    /// An import: in from the outside.
    In,
}

/// T-306: a uniform `[0, 1)` for the load `building` would send (`Way::Out`) or order
/// (`Way::In`) of `cargo` in the day's `bucket` (`tick / GOODS_BUCKET_TICKS`), against
/// `Balance::industry_export_share` / `industry_import_share`. A hash of the sim seed and the
/// keys, no RNG draw: the same for any call order, and a building's loads spread over the days.
pub fn trade_roll(
    seed: [u8; 32],
    building: BuildingId,
    bucket: u64,
    cargo: CargoKind,
    way: Way,
) -> f32 {
    let a = (u64::from(building.index) << 1) | u64::from(way == Way::In);
    let b = (bucket << 8) | cargo as u64;
    cp_core::hash_unit(seed, a, b)
}

/// Ticks between two buckets: a day in 64 parts.
pub const GOODS_BUCKET_TICKS: u64 = TICKS_PER_DAY / 64;

/// One end of a haul: a building, or the `Outside` node of an entry highway (T-170).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Endpoint {
    Building(BuildingId),
    Outside(NodeId),
}

/// A truck on its way with `load` units: factory to shop, factory to the outside (export) or
/// the outside to a shop (import).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Haul {
    pub from: Endpoint,
    pub to: Endpoint,
    pub load: f32,
    /// What the truck carries; `Goods` for the shop goods of M3. Added in v44 (T-210).
    pub cargo: CargoKind,
}

/// A truck of an industrial cargo that got there (T-210): `cp_industry` unloads it. Trucks with
/// `CargoKind::Goods` are handled here. Not saved: `cp_industry` drains the list in the same
/// tick, right after [`run_goods`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CargoArrival {
    pub cargo: CargoKind,
    pub load: f32,
    pub from: Endpoint,
    pub to: Endpoint,
    /// The price factor of the goods (1, or `factory_export_mul` from a unique factory).
    pub mul: f32,
}

/// The cargo trucks that arrived this tick, in run-id order.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct CargoArrivals {
    pub list: Vec<CargoArrival>,
}

impl Goods {
    /// Starts a truck with `load` units of an industrial `cargo` (T-210); the caller has taken
    /// the load out of its own stock. Returns the run.
    pub fn start_cargo_haul(
        &mut self,
        runs: &mut ServiceRuns,
        cargo: CargoKind,
        from: Endpoint,
        to: Endpoint,
        load: f32,
    ) -> RunId {
        let point = |e: Endpoint| match e {
            Endpoint::Building(b) => RunPoint::Building(b),
            Endpoint::Outside(n) => RunPoint::Outside(n),
        };
        let run = runs.start_run(VehicleKind::for_cargo(cargo), point(from), point(to), false);
        self.hauls.insert(
            run,
            Haul {
                from,
                to,
                load,
                cargo,
            },
        );
        run
    }
}

/// Stock of the factories and shops and the trucks under way. Serialized sim state.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Goods {
    pub stock: BTreeMap<BuildingId, f32>,
    /// Since when a shop has had no stock.
    pub empty_since: BTreeMap<BuildingId, Tick>,
    pub hauls: BTreeMap<RunId, Haul>,
}

impl Goods {
    pub fn is_empty(&self) -> bool {
        self.stock.is_empty() && self.empty_since.is_empty() && self.hauls.is_empty()
    }

    pub fn stock_of(&self, b: BuildingId) -> f32 {
        self.stock.get(&b).copied().unwrap_or(0.0)
    }
}

/// `Agents` phase, after `run_tourists`: lands the trucks that arrived, then runs the bucket of
/// this tick.
#[allow(clippy::too_many_arguments)]
pub fn run_goods(
    tick: Res<Tick>,
    rng: Res<cp_core::SimRng>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    citizens: Res<CitizenGrid>,
    mut buildings: ResMut<BuildingGrid>,
    mut goods: ResMut<Goods>,
    mut ledger: ResMut<TransitLedger>,
    mut runs: ResMut<ServiceRuns>,
    mut agents: ResMut<Agents>,
    mut arrivals: ResMut<CargoArrivals>,
    transit: Res<crate::model::TransitNet>,
    market: Option<Res<cp_content::Market>>,
    rail: Res<cp_net::RailReach>,
    mut hubs: ResMut<crate::hubs::CargoHubs>,
) {
    arrivals.list.clear();
    let Some(content) = content else { return };
    let db = &*content.0;
    let bal = db.balance();
    let now = *tick;
    let fallback = cp_content::Market::default();
    let market = market.as_deref().unwrap_or(&fallback);

    // 1. Trucks: arrived (unload), failed or lost (the load is gone).
    let ids: Vec<RunId> = goods.hauls.keys().copied().collect();
    for id in ids {
        match runs.get(id).map(|r| r.state) {
            Some(RunState::Arrived { .. } | RunState::Parked { .. }) => {
                let h = goods.hauls.remove(&id).expect("listed");
                let to_hub = matches!(h.to, Endpoint::Building(t)
                    if buildings.get(t).is_some_and(|b| is_hub_def(db, b.def)));
                if h.cargo != CargoKind::Goods || to_hub {
                    // An industrial cargo: `cp_industry` unloads it.
                    let mul = match h.from {
                        Endpoint::Building(f)
                            if h.cargo == CargoKind::Goods
                                && buildings.get(f).is_some_and(|b| {
                                    db.building(b.def)
                                        .industry
                                        .as_ref()
                                        .is_some_and(|i| i.role == IndustryRole::Factory)
                                }) =>
                        {
                            bal.factory_export_mul
                        }
                        _ => 1.0,
                    };
                    arrivals.list.push(CargoArrival {
                        cargo: h.cargo,
                        load: h.load,
                        from: h.from,
                        to: h.to,
                        mul,
                    });
                    runs.release(id, &mut agents);
                    continue;
                }
                match h.to {
                    Endpoint::Building(to) if buildings.get(to).is_some() => {
                        let from_outside = matches!(h.from, Endpoint::Outside(_));
                        // T-306: a factory's raw materials from the outside are used up on
                        // arrival; only a shop's import is stock.
                        let raw = from_outside
                            && buildings
                                .get(to)
                                .is_some_and(|t| db.building(t.def).zone == ZoneType::Industrial);
                        if !raw {
                            let s = goods.stock.entry(to).or_default();
                            *s = (*s + h.load).min(bal.goods_stock_cap);
                        }
                        if from_outside {
                            ledger.import_loads += 1;
                        }
                    }
                    Endpoint::Building(_) => {}
                    Endpoint::Outside(_) => {
                        // The goods of a unique factory sell for more (T-215).
                        let mul = match h.from {
                            Endpoint::Building(f)
                                if buildings.get(f).is_some_and(|b| {
                                    db.building(b.def)
                                        .industry
                                        .as_ref()
                                        .is_some_and(|i| i.role == IndustryRole::Factory)
                                }) =>
                            {
                                bal.factory_export_mul
                            }
                            _ => 1.0,
                        };
                        let price = market.export_price(db, CargoKind::Goods, now);
                        ledger.exports += Cost::scaled(price * mul, f64::from(h.load));
                        ledger.export_loads += 1;
                    }
                }
                runs.release(id, &mut agents);
            }
            Some(RunState::Failed) | None => {
                goods.hauls.remove(&id);
                runs.release(id, &mut agents);
            }
            Some(_) => {}
        }
    }

    // 2. The bucket of this tick.
    if !now.0.is_multiple_of(GOODS_BUCKET_TICKS) {
        return;
    }
    let bucket = (now.0 / GOODS_BUCKET_TICKS) % 64;
    goods.stock.retain(|id, _| buildings.get(*id).is_some());
    goods
        .empty_since
        .retain(|id, _| buildings.get(*id).is_some());

    let mut workers: Vec<u32> = Vec::new();
    for (_, c) in citizens.citizens() {
        if let Some(w) = c.work {
            let i = w.index as usize;
            if workers.len() <= i {
                workers.resize(i + 1, 0);
            }
            workers[i] += 1;
        }
    }
    let filled = |id: BuildingId| workers.get(id.index as usize).copied().unwrap_or(0) as f32;
    let empty_after = (bal.goods_empty_days * TICKS_PER_DAY as f32) as u64;

    // The entry highways' `Outside` nodes (not the railway's): an import starts where a truck can
    // drive into the city, an export ends where one can drive out (BUG-095: the nearest node of
    // either carriageway sent half the trucks the wrong way, and they never got there).
    let (entries, exits) = cp_net::outside_ways(&net, db);
    // The working cargo stations (T-216): the exports and imports go through the nearest of them
    // and the entry highways.
    let sites = hub_sites(db, &transit, &rail, &buildings);
    let train_wait = (bal.cargo_train_interval_h * TICKS_PER_HOUR)
        .ceil()
        .max(1.0) as u64;
    let point = |e: Endpoint| match e {
        Endpoint::Building(b) => RunPoint::Building(b),
        Endpoint::Outside(n) => RunPoint::Outside(n),
    };
    // T-306: traffic grows with industry. A hash of the factory, the bucket and the cargo picks
    // the `industry_export_share` of its full loads that go out by the entry highway even with a
    // shop in reach (the same keys as `cp_industry`'s freight, so the two never disagree).
    let seed = rng.seed();
    let imports_today = |id: BuildingId| {
        trade_roll(
            seed,
            id,
            now.0 / GOODS_BUCKET_TICKS,
            CargoKind::Goods,
            Way::In,
        ) < bal.industry_import_share
    };
    let exports_this_load = |id: BuildingId| {
        trade_roll(
            seed,
            id,
            now.0 / GOODS_BUCKET_TICKS,
            CargoKind::Goods,
            Way::Out,
        ) < bal.industry_export_share
    };

    let mine: Vec<BuildingId> = buildings
        .iter()
        .filter(|(id, b)| u64::from(id.index) % 64 == bucket && b.state == BuildingState::Active)
        .map(|(id, _)| id)
        .collect();
    // Built the first time a factory or shop of the bucket asks (T-291).
    let mut lookup: Option<Lookup> = None;
    for id in mine {
        let b = buildings.get(id).expect("listed");
        let zone = db.building(b.def).zone;
        // The buildings of the industrial chains make their own cargo (cp_industry, T-209); only
        // the unique factories (T-215) sell shop goods.
        if db
            .building(b.def)
            .industry
            .as_ref()
            .is_some_and(|i| i.role != IndustryRole::Factory)
        {
            continue;
        }
        match zone {
            // A service, park or landmark stands on an `Industrial` def (BUG-095): it makes no
            // shop goods (an industry chain building may name a service too: it keeps its role).
            ZoneType::Industrial
                if db.building(b.def).industry.is_none()
                    && (db.building(b.def).service.is_some()
                        || db.building(b.def).park.is_some()
                        || db.building(b.def).unique.is_some()) => {}
            ZoneType::Industrial => {
                let s = goods.stock.entry(id).or_default();
                // A unique factory (T-215) puts its Goods into the stock itself; the chain
                // buildings make no shop goods.
                if db.building(b.def).industry.is_none() {
                    *s = (*s + bal.goods_per_worker * filled(id)).min(bal.goods_stock_cap);
                }
                let stock = *s;
                let full = stock >= bal.goods_truck_load;
                // A small factory may send a part load (BUG-058) to a shop that stands empty.
                let partial = !full && stock >= bal.goods_truck_load * PARTIAL_LOAD_SHARE;
                let lk = lookup.get_or_insert_with(|| Lookup::new(&buildings, db, &goods));
                if (full || partial) && b.access().is_some() && !lk.from.contains(&id) {
                    let load = stock.min(bal.goods_truck_load);
                    // Nobody to sell to nearby: the goods go out of the country. T-306: a share
                    // of the loads, part loads too, goes out whatever shop stands near.
                    let abroad = || exit_for(&exits, &sites, b.pos).map(|e| (e, point(e)));
                    let exported = exports_this_load(id);
                    let to = if partial {
                        let empty = pick_empty_shop(lk, &goods, b.pos, bal.freight_radius_m)
                            .map(|shop| (Endpoint::Building(shop), RunPoint::Building(shop)));
                        if exported { abroad().or(empty) } else { empty }
                    } else {
                        let shop = pick_shop(lk, &goods, b.pos, bal.freight_radius_m)
                            .map(|shop| (Endpoint::Building(shop), RunPoint::Building(shop)));
                        if exported {
                            abroad().or(shop)
                        } else {
                            shop.or_else(abroad)
                        }
                    };
                    if let Some((to, point)) = to {
                        let run = runs.start_run(
                            VehicleKind::Cargo,
                            RunPoint::Building(id),
                            point,
                            false,
                        );
                        *goods.stock.get_mut(&id).expect("just set") -= load;
                        let haul = Haul {
                            from: Endpoint::Building(id),
                            to,
                            load,
                            cargo: CargoKind::Goods,
                        };
                        lk.note(&haul);
                        goods.hauls.insert(run, haul);
                    }
                }
                // T-306: a working factory gets a share of its raw materials by truck from the
                // outside: once a day (its bucket), when the roll says so and no raw truck is on
                // its way, a `Cargo` run from the entry highway. The load is used up on arrival
                // (no stock) and counted in `TransitLedger::import_loads`; free for the treasury,
                // as a shop's import (T-170). The chains' factories import through `cp_industry`.
                if db.building(b.def).industry.is_none()
                    && filled(id) > 0.0
                    && b.access().is_some()
                    && !lk.to.contains(&id)
                    && imports_today(id)
                    && let Some(Endpoint::Outside(entry)) = exit_for(&entries, &sites, b.pos)
                {
                    let run = runs.start_run(
                        VehicleKind::Cargo,
                        RunPoint::Outside(entry),
                        RunPoint::Building(id),
                        false,
                    );
                    let haul = Haul {
                        from: Endpoint::Outside(entry),
                        to: Endpoint::Building(id),
                        load: bal.goods_truck_load,
                        cargo: CargoKind::Goods,
                    };
                    lk.note(&haul);
                    goods.hauls.insert(run, haul);
                }
            }
            ZoneType::ComLow | ZoneType::ComHigh => {
                let s = goods.stock.entry(id).or_default();
                // A specialised shop uses goods at its own rate (T-196, Organic).
                let mul = db
                    .building(b.def)
                    .spec
                    .and_then(|k| db.spec(k))
                    .map_or(1.0, |s| s.goods_mul);
                *s = (*s - bal.goods_per_job * filled(id) * mul).max(0.0);
                let empty = *s <= 0.0;
                let low = *s < bal.goods_import_threshold;
                let b = buildings.get(id).expect("listed");
                let lk = lookup.get_or_insert_with(|| Lookup::new(&buildings, db, &goods));
                // No factory within reach, stock running out: a truck from the outside.
                if low
                    && b.access().is_some()
                    && !lk.to.contains(&id)
                    && !hubs.pending(id, CargoKind::Goods)
                    && !has_stocked_supplier(lk, &goods, b.pos, bal)
                    && let Some(via) = exit_for(&entries, &sites, b.pos)
                {
                    match via {
                        // By rail: the train brings it, a truck takes it on from the station.
                        Endpoint::Building(hub) => hubs.order(
                            hub,
                            CargoKind::Goods,
                            id,
                            bal.goods_truck_load,
                            now,
                            train_wait,
                        ),
                        Endpoint::Outside(entry) => {
                            let run = runs.start_run(
                                VehicleKind::Cargo,
                                RunPoint::Outside(entry),
                                RunPoint::Building(id),
                                false,
                            );
                            let haul = Haul {
                                from: Endpoint::Outside(entry),
                                to: Endpoint::Building(id),
                                load: bal.goods_truck_load,
                                cargo: CargoKind::Goods,
                            };
                            lk.note(&haul);
                            goods.hauls.insert(run, haul);
                        }
                    }
                }
                if empty {
                    let since = goods.empty_since.entry(id).or_insert(now).0;
                    if now.0 - since >= empty_after {
                        buildings.set_flag(id, BuildingFlags::NO_GOODS, true);
                    }
                } else {
                    goods.empty_since.remove(&id);
                    buildings.set_flag(id, BuildingFlags::NO_GOODS, false);
                }
            }
            _ => {}
        }
    }
}

/// What the factories and shops of one bucket look up again and again, built once for it
/// (T-291: every lookup walked all the buildings and all the trucks): the shops and the goods
/// factories with their positions, in id order, and the buildings a truck (of any cargo) is on its
/// way from or to, kept up as the bucket sends trucks. The buildings do not change during the
/// bucket (only their flags); the stocks are read when asked.
struct Lookup {
    shops: Vec<(BuildingId, Vec2)>,
    factories: Vec<(BuildingId, Vec2)>,
    from: BTreeSet<BuildingId>,
    to: BTreeSet<BuildingId>,
}

impl Lookup {
    fn new(buildings: &BuildingGrid, db: &cp_content::ContentDb, goods: &Goods) -> Self {
        let mut shops = Vec::new();
        let mut factories = Vec::new();
        for (id, b) in buildings.iter() {
            if b.state != BuildingState::Active || b.access().is_none() {
                continue;
            }
            let def = db.building(b.def);
            match def.zone {
                ZoneType::ComLow | ZoneType::ComHigh if def.industry.is_none() => {
                    shops.push((id, b.pos));
                }
                ZoneType::Industrial
                    if def
                        .industry
                        .as_ref()
                        .is_none_or(|i| i.role == IndustryRole::Factory) =>
                {
                    factories.push((id, b.pos));
                }
                _ => {}
            }
        }
        let mut lk = Self {
            shops,
            factories,
            from: BTreeSet::new(),
            to: BTreeSet::new(),
        };
        for h in goods.hauls.values() {
            lk.note(h);
        }
        lk
    }

    fn note(&mut self, h: &Haul) {
        if let Endpoint::Building(b) = h.from {
            self.from.insert(b);
        }
        if let Endpoint::Building(b) = h.to {
            self.to.insert(b);
        }
    }
}

/// The shop with the least stock within `radius` of `origin` (smaller id on a tie).
fn pick_shop(lk: &Lookup, goods: &Goods, origin: Vec2, radius: f32) -> Option<BuildingId> {
    let mut best: Option<(f32, BuildingId)> = None;
    for &(id, pos) in &lk.shops {
        if pos.distance(origin) > radius {
            continue;
        }
        let s = goods.stock_of(id);
        if best.is_none_or(|(bs, _)| s < bs) {
            best = Some((s, id));
        }
    }
    best.map(|(_, id)| id)
}

/// The shop within `radius` of `origin` that has no stock and no truck on its way, nearest first
/// (smaller id on a tie).
fn pick_empty_shop(lk: &Lookup, goods: &Goods, origin: Vec2, radius: f32) -> Option<BuildingId> {
    lk.shops
        .iter()
        .filter(|(id, pos)| {
            pos.distance(origin) <= radius && goods.stock_of(*id) <= 0.0 && !lk.to.contains(id)
        })
        .min_by(|a, b| {
            a.1.distance(origin)
                .total_cmp(&b.1.distance(origin))
                .then(a.0.cmp(&b.0))
        })
        .map(|&(id, _)| id)
}

/// Whether an active factory with a road stands within the delivery radius of a shop at
/// `origin` and could send a load soon: it holds a truck load, or a part load while the shop is
/// empty (BUG-058). A weak factory that cannot fill a truck does not block imports.
fn has_stocked_supplier(
    lk: &Lookup,
    goods: &Goods,
    origin: Vec2,
    bal: &cp_content::Balance,
) -> bool {
    lk.factories.iter().any(|&(id, pos)| {
        pos.distance(origin) <= bal.freight_radius_m
            && goods.stock_of(id) >= bal.goods_truck_load * PARTIAL_LOAD_SHARE
    })
}

//! Cargo by truck along the chain (T-210, `architecture/system-design.md` §16 M5 (D)).
//!
//! Once a day (the same 64 buckets as production), a building of a chain that holds a truck load
//! (`Balance::goods_truck_load`) of one of its outputs sends it, if no truck of that cargo is on
//! the way from it: first to the processor or factory within `freight_radius_m` that needs the
//! cargo and has the least of it (a tie goes to the smaller `BuildingId`), then to a warehouse of
//! its chain with room, and else out of the map by the entry highway (`export_price × load` in
//! the [`IndustryLedger`] when the truck arrives). A processor short of an input that no
//! building within the radius can supply gets a truck from the outside (`import_price × load`,
//! booked when it sets out). Trucks are the `Cargo` runs of `cp_traffic`; with the vehicle pool
//! full a run arrives abstractly, as in M3. Distance is the straight line, as for the goods.
//! `cp_economy` books the ledger at the end of the week.
//!
//! **Traffic grows with industry (T-306).** Whatever stands near, `industry_export_share` of a
//! building's outgoing loads (an extractor's, a processor's, a warehouse's that may give) go out
//! by the entry highway, and `industry_import_share` of a processor's or factory's input loads
//! are ordered from the outside: a hash of the building, the day's bucket and the cargo
//! ([`cp_transit::trade_roll`]) picks them. More industry, more trucks through the entry both
//! ways; the playtest prints them against the industrial job count.

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut, SystemParam};
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{CargoKind, IndustryRole, SimContent};
use cp_core::Cost;
use cp_core::Tick;
use cp_core::time::TICKS_PER_DAY;
use cp_district::{AreaMap, Areas, area_at};
use cp_net::NetGraph;
use cp_traffic::ServiceRuns;
use cp_transit::{
    CargoArrivals, CargoHubs, Endpoint, Goods, Way, exit_for, hub_sites, is_hub_def, trade_roll,
};
use serde::{Deserialize, Serialize};

use crate::warehouse::{WarehouseMode, WarehouseSettings};
use crate::{INDUSTRY_BUCKET_TICKS, IndustryState, storage_cap};

/// The money of the cargo trade: this week's sums and what the last settled week booked.
/// Saved. `cp_economy::settle_week` books the week at its end.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndustryLedger {
    /// What the trucks that left the map brought this week.
    pub exports: Cost,
    /// What the trucks bought from the outside cost this week.
    pub imports: Cost,
    pub export_loads: u32,
    pub import_loads: u32,
    /// The last settled week.
    pub last_exports: Cost,
    pub last_imports: Cost,
    /// Units sold to and bought from the outside this week per cargo, in [`CargoKind`] order
    /// (§21.2: the Trade table), and the last settled week's.
    pub by_cargo: [CargoUnits; 13],
    pub last_by_cargo: [CargoUnits; 13],
}

/// Units of one cargo that went out of and came into the city.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CargoUnits {
    pub export: u32,
    pub import: u32,
}

impl IndustryLedger {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Books the week: the sums become `last_*` and start over; returns `(exports, imports)`.
    pub fn settle_week(&mut self) -> (Cost, Cost) {
        let booked = (self.exports, self.imports);
        *self = Self {
            last_exports: self.exports,
            last_imports: self.imports,
            last_by_cargo: self.by_cargo,
            ..Self::default()
        };
        booked
    }

    /// Counts `export` units of `cargo` out and `import` in (rounded).
    pub fn count(&mut self, cargo: CargoKind, export: f32, import: f32) {
        let u = &mut self.by_cargo[cargo as usize];
        u.export = u.export.saturating_add(export.round().max(0.0) as u32);
        u.import = u.import.saturating_add(import.round().max(0.0) as u32);
    }
}

/// One industrial area's trade and output over a week (T-316).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AreaBook {
    /// Units made by the area's extractors, processors and factories, by cargo.
    pub produced: BTreeMap<CargoKind, f32>,
    /// What the area's trucks brought in from outside the map.
    pub exports: Cost,
    /// What the area's orders from the outside cost, transport fee included.
    pub imports: Cost,
}

/// The per-area books of the cargo trade: the current game week and the last one, by area id.
/// Saved. The week rolls itself over when the tick passes a week boundary ([`Self::roll`]), so
/// no other system settles it.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AreaBooks {
    /// The week `now` holds (`tick / TICKS_PER_WEEK`).
    pub week: u64,
    pub now: BTreeMap<u8, AreaBook>,
    pub last: BTreeMap<u8, AreaBook>,
    /// The last settled week's worker taxes and building upkeep of each area's chain
    /// buildings, `(taxes, upkeep)`; set by `cp_economy`'s weekly settlement.
    pub money: BTreeMap<u8, (Cost, Cost)>,
}

impl AreaBooks {
    pub fn is_empty(&self) -> bool {
        self.now.is_empty() && self.last.is_empty() && self.money.is_empty()
    }

    /// Starts a new week when `tick` is in a later one than `now`'s: `now` becomes `last`.
    pub fn roll(&mut self, tick: Tick) {
        let week = tick.0 / cp_core::time::TICKS_PER_WEEK;
        if week > self.week {
            // More than one week skipped: the week before was empty.
            self.last = if week == self.week + 1 {
                std::mem::take(&mut self.now)
            } else {
                self.now.clear();
                BTreeMap::new()
            };
            self.week = week;
        }
    }

    pub fn book(&mut self, area: u8) -> &mut AreaBook {
        self.now.entry(area).or_default()
    }

    /// The area's last settled week.
    pub fn last_of(&self, area: u8) -> Option<&AreaBook> {
        self.last.get(&area)
    }
}

/// The ways in and out of the city and the sim seed (T-306), bundled so [`run_freight`] stays
/// within `bevy_ecs`'s system parameter count.
#[derive(SystemParam)]
pub struct Via<'w> {
    rng: Res<'w, cp_core::SimRng>,
    transit: Res<'w, cp_transit::TransitNet>,
    rail: Res<'w, cp_net::RailReach>,
    hubs: ResMut<'w, CargoHubs>,
    market: Option<Res<'w, cp_content::Market>>,
}

/// Unloads the trucks that arrived and sends the loads of this tick's bucket.
#[allow(clippy::too_many_arguments)]
pub fn run_freight(
    tick: Res<Tick>,
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    citizens: Res<CitizenGrid>,
    buildings: Res<BuildingGrid>,
    area_map: Res<AreaMap>,
    areas: Res<Areas>,
    arrivals: Res<CargoArrivals>,
    mut goods: ResMut<Goods>,
    mut runs: ResMut<ServiceRuns>,
    mut state: ResMut<IndustryState>,
    mut ledger: ResMut<IndustryLedger>,
    mut books: ResMut<AreaBooks>,
    mut settings: ResMut<WarehouseSettings>,
    mut via: Via,
) {
    let Via {
        rng,
        transit,
        rail,
        hubs,
        market,
    } = &mut via;
    let (transit, rail, rng) = (&**transit, &**rail, &**rng);
    let fallback = cp_content::Market::default();
    let market = market.as_deref().unwrap_or(&fallback);
    let Some(content) = content else { return };
    let db = &*content.0;
    let bal = db.balance();

    settings.list.retain(|id, _| buildings.get(*id).is_some());
    books.roll(*tick);

    let train_wait = (bal.cargo_train_interval_h * cp_core::time::TICKS_PER_HOUR)
        .ceil()
        .max(1.0) as u64;
    // 1. Arrived: unload (a truck whose building is gone loses its load). A cargo station keeps
    // it for the next train (T-216).
    for a in &arrivals.list {
        match a.to {
            Endpoint::Building(b) if buildings.get(b).is_some_and(|x| is_hub_def(db, x.def)) => {
                hubs.deposit(
                    b,
                    a.cargo,
                    a.load,
                    a.load * (a.mul - 1.0),
                    *tick,
                    train_wait,
                );
            }
            Endpoint::Building(b) if buildings.get(b).is_some() => {
                state.add(b, a.cargo, a.load);
            }
            Endpoint::Building(_) => {}
            Endpoint::Outside(_) => {
                let price = market.export_price(db, a.cargo, *tick);
                let pay = Cost::scaled(price, f64::from(a.load));
                ledger.exports += pay;
                ledger.export_loads += 1;
                ledger.count(a.cargo, a.load, 0.0);
                if let Endpoint::Building(from) = a.from
                    && let Some(area) = buildings.get(from).and_then(|b| area_at(&area_map, b.pos))
                {
                    books.book(area.0).exports += pay;
                }
            }
        }
    }

    // 2. The bucket of this tick.
    if !tick.0.is_multiple_of(INDUSTRY_BUCKET_TICKS) {
        return;
    }
    let bucket = (tick.0 / INDUSTRY_BUCKET_TICKS) % 64;
    let load = bal.goods_truck_load;
    let radius = bal.freight_radius_m;

    // Imports start where a truck can drive into the city, exports end where one can drive out
    // (BUG-095).
    let (entries, exits) = cp_net::outside_ways(&net, db);
    // The working cargo stations: exports and imports go through the nearest of them and the
    // entry highways (T-216).
    let sites = hub_sites(db, transit, rail, &buildings);
    // T-306: traffic grows with industry. A hash of the building, the bucket and the cargo picks
    // the `industry_export_share` of a building's outgoing loads that go out by the entry
    // highway whatever stands near, and the `industry_import_share` of a processor's or
    // factory's input loads that come in from the outside whatever stands near.
    let seed = rng.seed();
    let roll = |id: BuildingId, cargo: CargoKind, way: Way| {
        trade_roll(seed, id, tick.0 / INDUSTRY_BUCKET_TICKS, cargo, way)
    };
    let goes_abroad = |id, cargo| roll(id, cargo, Way::Out) < bal.industry_export_share;
    let comes_from_abroad = |id, cargo| roll(id, cargo, Way::In) < bal.industry_import_share;

    // Cargo already on the way to a building, by building and cargo.
    let inbound = |goods: &Goods, to: BuildingId, cargo: CargoKind| -> f32 {
        goods
            .hauls
            .values()
            .filter(|h| h.to == Endpoint::Building(to) && h.cargo == cargo)
            .map(|h| h.load)
            .sum()
    };
    let cap_of = |id: BuildingId, storage: f32| -> f32 {
        let level = buildings
            .get(id)
            .and_then(|b| area_at(&area_map, b.pos))
            .and_then(|a| areas.get(a))
            .map_or(1, |a| a.level);
        storage_cap(storage, bal.industry_storage_per_level, level)
    };

    for (id, b) in buildings.iter() {
        if u64::from(id.index) % 64 != bucket || b.state != BuildingState::Active {
            continue;
        }
        let Some(spec) = &db.building(b.def).industry else {
            continue;
        };
        if b.access().is_none() {
            continue;
        }

        // Outgoing loads.
        if matches!(spec.role, IndustryRole::Extractor | IndustryRole::Processor) {
            for &(cargo, _) in &spec.outputs {
                if state.stock_of(id, cargo) < load
                    || goods
                        .hauls
                        .values()
                        .any(|h| h.from == Endpoint::Building(id) && h.cargo == cargo)
                {
                    continue;
                }
                // 1. A processor or factory that needs it; 2. a warehouse; 3. the outside.
                let mut best: Option<(f32, BuildingId)> = None;
                let mut shelf: Option<(f32, BuildingId)> = None;
                for (tid, t) in buildings.iter() {
                    if tid == id || t.state != BuildingState::Active || t.access().is_none() {
                        continue;
                    }
                    let Some(ts) = &db.building(t.def).industry else {
                        continue;
                    };
                    if t.pos.distance(b.pos) > radius {
                        continue;
                    }
                    let held = state.stock_of(tid, cargo) + inbound(&goods, tid, cargo);
                    let room = cap_of(tid, ts.storage) - state.total(tid);
                    match ts.role {
                        IndustryRole::Processor | IndustryRole::Factory
                            if ts.inputs.iter().any(|&(c, _)| c == cargo)
                                && room >= load
                                && best.is_none_or(|(s, _)| held < s) =>
                        {
                            best = Some((held, tid));
                        }
                        IndustryRole::Warehouse
                            if ts.resource == spec.resource
                                && room - inbound(&goods, tid, cargo) >= load
                                && settings.get(tid).accepts(
                                    cargo,
                                    state.total(tid),
                                    cap_of(tid, ts.storage),
                                )
                                && shelf.is_none_or(|(s, _)| held < s) =>
                        {
                            shelf = Some((held, tid));
                        }
                        _ => {}
                    }
                }
                let near = best.or(shelf).map(|(_, t)| Endpoint::Building(t));
                let to = if goes_abroad(id, cargo) {
                    exit_for(&exits, &sites, b.pos).or(near)
                } else {
                    near.or_else(|| exit_for(&exits, &sites, b.pos))
                };
                if let Some(to) = to {
                    state.add(id, cargo, -load);
                    goods.start_cargo_haul(&mut runs, cargo, Endpoint::Building(id), to, load);
                }
            }
        }

        // A warehouse gives what its mode lets it give: to a processor that is short of it,
        // and in the Empty mode out of the map.
        if spec.role == IndustryRole::Warehouse {
            let set = settings.get(id);
            let cap = cap_of(id, spec.storage);
            let held_all = state.total(id);
            let mine: Vec<(CargoKind, f32)> = state.stock.get(&id).cloned().unwrap_or_default();
            for (cargo, amount) in mine {
                if amount < load
                    || !set
                        .in_market(market.shortage(cargo))
                        .gives(cargo, held_all, cap)
                    || goods
                        .hauls
                        .values()
                        .any(|h| h.from == Endpoint::Building(id) && h.cargo == cargo)
                {
                    continue;
                }
                let mut best: Option<(f32, BuildingId)> = None;
                for (tid, t) in buildings.iter() {
                    if tid == id || t.state != BuildingState::Active || t.access().is_none() {
                        continue;
                    }
                    let Some(ts) = &db.building(t.def).industry else {
                        continue;
                    };
                    if !matches!(ts.role, IndustryRole::Processor | IndustryRole::Factory)
                        || !ts.inputs.iter().any(|&(c, _)| c == cargo)
                        || t.pos.distance(b.pos) > radius
                    {
                        continue;
                    }
                    let held = state.stock_of(tid, cargo) + inbound(&goods, tid, cargo);
                    let room = cap_of(tid, ts.storage) - state.total(tid);
                    if held < load && room >= load && best.is_none_or(|(s, _)| held < s) {
                        best = Some((held, tid));
                    }
                }
                // T-306: a warehouse that may give (not `Fill`) sends its share abroad too; a
                // `Fill` one giving in a shortage keeps it for the city.
                let near = best.map(|(_, t)| Endpoint::Building(t));
                let to = if set.mode != WarehouseMode::Fill && goes_abroad(id, cargo) {
                    exit_for(&exits, &sites, b.pos).or(near)
                } else {
                    near.or_else(|| {
                        (set.mode == WarehouseMode::Empty)
                            .then(|| exit_for(&exits, &sites, b.pos))
                            .flatten()
                    })
                };
                if let Some(to) = to {
                    state.add(id, cargo, -load);
                    goods.start_cargo_haul(&mut runs, cargo, Endpoint::Building(id), to, load);
                }
            }
        }

        // Missing input, and nobody near to make it: a truck from the outside.
        if matches!(spec.role, IndustryRole::Processor | IndustryRole::Factory) {
            let slots: u32 = db.building(b.def).jobs.iter().map(|&j| u32::from(j)).sum();
            let filled: u32 = (0..4).map(|l| u32::from(citizens.jobs_filled(id, l))).sum();
            if slots > 0 && filled == 0 {
                continue;
            }
            for &(cargo, _) in &spec.inputs {
                if state.stock_of(id, cargo) >= load
                    || inbound(&goods, id, cargo) > 0.0
                    || hubs.pending(id, cargo)
                    || cap_of(id, spec.storage) - state.total(id) < load
                {
                    continue;
                }
                let supplied = buildings.iter().any(|(sid, s)| {
                    sid != id
                        && s.state == BuildingState::Active
                        && s.pos.distance(b.pos) <= radius
                        && db
                            .building(s.def)
                            .industry
                            .as_ref()
                            .is_some_and(|ss| match ss.role {
                                IndustryRole::Extractor
                                | IndustryRole::Processor
                                | IndustryRole::Factory => {
                                    ss.outputs.iter().any(|&(c, _)| c == cargo)
                                }
                                IndustryRole::Warehouse => {
                                    state.stock_of(sid, cargo) > 0.0
                                        && settings
                                            .get(sid)
                                            .in_market(market.shortage(cargo))
                                            .gives(cargo, state.total(sid), cap_of(sid, ss.storage))
                                }
                                _ => false,
                            })
                });
                // T-306: a share of the input loads comes from abroad even with a supplier near.
                if supplied && !comes_from_abroad(id, cargo) {
                    continue;
                }
                // §21.1 item 3: in a shortage a share of the orders fails, free; the building
                // orders again on its next pass.
                if market.import_fails(&bal.market, seed, id.index, cargo, tick.0 / TICKS_PER_DAY) {
                    continue;
                }
                if let Some(via) = exit_for(&entries, &sites, b.pos) {
                    let price = market.import_price(db, cargo, *tick);
                    let cost = Cost::scaled(price, f64::from(load))
                        + Cost::scaled(bal.import_transport_fee, 1.0);
                    ledger.imports += cost;
                    if let Some(area) = area_at(&area_map, b.pos) {
                        books.book(area.0).imports += cost;
                    }
                    ledger.import_loads += 1;
                    ledger.count(cargo, 0.0, load);
                    match via {
                        // By rail: ordered at the station, the train brings it (T-216).
                        Endpoint::Building(hub) => {
                            hubs.order(hub, cargo, id, load, *tick, train_wait);
                        }
                        Endpoint::Outside(entry) => {
                            goods.start_cargo_haul(
                                &mut runs,
                                cargo,
                                Endpoint::Outside(entry),
                                Endpoint::Building(id),
                                load,
                            );
                        }
                    }
                }
            }
        }
    }
}

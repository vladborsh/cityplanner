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

use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{CargoKind, IndustryRole, SimContent};
use cp_core::Cost;
use cp_core::Tick;
use cp_district::{AreaMap, Areas, area_at};
use cp_net::{NetGraph, NodeId, NodeKind, node_is_rail};
use cp_traffic::ServiceRuns;
use cp_transit::{CargoArrivals, CargoHubs, Endpoint, Goods, exit_for, hub_sites, is_hub_def};
use serde::{Deserialize, Serialize};

use crate::warehouse::WarehouseSettings;
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
            ..Self::default()
        };
        booked
    }
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
    mut settings: ResMut<WarehouseSettings>,
    transit: Res<cp_transit::TransitNet>,
    rail: Res<cp_net::RailReach>,
    mut hubs: ResMut<CargoHubs>,
) {
    let Some(content) = content else { return };
    let db = &*content.0;
    let bal = db.balance();

    settings.list.retain(|id, _| buildings.get(*id).is_some());

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
                let price = db.cargo(a.cargo).map_or(0.0, |c| c.export_price);
                ledger.exports += Cost::scaled(price, f64::from(a.load));
                ledger.export_loads += 1;
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

    let entries: Vec<(NodeId, Vec2)> = net
        .nodes()
        .filter(|(_, n)| {
            n.kind == NodeKind::Outside && !n.segments.is_empty() && !node_is_rail(&net, n)
        })
        .map(|(id, n)| (id, Vec2::new(n.pos.x, n.pos.z)))
        .collect();
    // The working cargo stations: exports and imports go through the nearest of them and the
    // entry highways (T-216).
    let sites = hub_sites(db, &transit, &rail, &buildings);

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
                let to = best
                    .or(shelf)
                    .map(|(_, t)| Endpoint::Building(t))
                    .or_else(|| exit_for(&entries, &sites, b.pos));
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
                    || !set.gives(cargo, held_all, cap)
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
                let to = best.map(|(_, t)| Endpoint::Building(t)).or_else(|| {
                    (set.mode == crate::warehouse::WarehouseMode::Empty)
                        .then(|| exit_for(&entries, &sites, b.pos))
                        .flatten()
                });
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
                let supplied =
                    buildings.iter().any(|(sid, s)| {
                        sid != id
                            && s.state == BuildingState::Active
                            && s.pos.distance(b.pos) <= radius
                            && db.building(s.def).industry.as_ref().is_some_and(|ss| {
                                match ss.role {
                                    IndustryRole::Extractor
                                    | IndustryRole::Processor
                                    | IndustryRole::Factory => {
                                        ss.outputs.iter().any(|&(c, _)| c == cargo)
                                    }
                                    IndustryRole::Warehouse => {
                                        state.stock_of(sid, cargo) > 0.0
                                            && settings.get(sid).gives(
                                                cargo,
                                                state.total(sid),
                                                cap_of(sid, ss.storage),
                                            )
                                    }
                                    _ => false,
                                }
                            })
                    });
                if supplied {
                    continue;
                }
                if let Some(via) = exit_for(&entries, &sites, b.pos) {
                    let price = db.cargo(cargo).map_or(0.0, |c| c.import_price);
                    ledger.imports += Cost::scaled(price, f64::from(load));
                    ledger.import_loads += 1;
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

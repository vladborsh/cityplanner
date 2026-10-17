//! Goods (§16 M3 D, T-169): factories make them, shops sell them, trucks carry them between.
//!
//! Once a day, in 64 buckets by `BuildingId.index % 64` (one bucket every 64 ticks), an `Active`
//! industrial building adds `goods_per_worker × filled jobs` to its stock (up to
//! `goods_stock_cap`) and a commercial one takes `goods_per_job × filled jobs` off. A factory
//! holding a `goods_truck_load` sends a truck (a `Cargo` run of `cp_traffic`) to the shop with the
//! least stock within `freight_radius_m` (straight-line distance; a tie goes to the smaller
//! `BuildingId`); the load leaves the factory at once and is added to the shop when the truck is
//! there (with the vehicle pool full the run arrives abstractly). A shop that has stood empty for
//! `goods_empty_days` gets `NO_GOODS`; a delivery clears it. A run that fails (the shop was
//! demolished on the way, no route) loses its load.
//!
//! **Exports and imports (T-170).** A factory holding a load with no shop within the radius sends
//! the truck to the nearest `Outside` node of a highway instead: the load leaves the map and
//! `export_price × load` goes to the [`TransitLedger`] (and on to the week report) when the truck
//! is there. A shop below `goods_import_threshold` with no factory within the radius gets a truck
//! from the nearest `Outside` node, free for the treasury (it is traffic on the entry road). With
//! no `Outside` node nothing is exported or imported, as before.

use std::collections::BTreeMap;

use crate::fleet::TransitLedger;
use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut};
use bevy_math::Vec2;
use cp_building::{BuildingFlags, BuildingGrid, BuildingId, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{SimContent, VehicleKind, ZoneType};
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Cost, Tick};
use cp_net::{NetGraph, NodeId, NodeKind, node_is_rail};
use cp_traffic::{Agents, RunId, RunPoint, RunState, ServiceRuns};
use serde::{Deserialize, Serialize};

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
    content: Option<Res<SimContent>>,
    net: Res<NetGraph>,
    citizens: Res<CitizenGrid>,
    mut buildings: ResMut<BuildingGrid>,
    mut goods: ResMut<Goods>,
    mut ledger: ResMut<TransitLedger>,
    mut runs: ResMut<ServiceRuns>,
    mut agents: ResMut<Agents>,
) {
    let Some(content) = content else { return };
    let db = &*content.0;
    let bal = db.balance();
    let now = *tick;

    // 1. Trucks: arrived (unload), failed or lost (the load is gone).
    let ids: Vec<RunId> = goods.hauls.keys().copied().collect();
    for id in ids {
        match runs.get(id).map(|r| r.state) {
            Some(RunState::Arrived { .. } | RunState::Parked { .. }) => {
                let h = goods.hauls.remove(&id).expect("listed");
                match h.to {
                    Endpoint::Building(to) if buildings.get(to).is_some() => {
                        let s = goods.stock.entry(to).or_default();
                        *s = (*s + h.load).min(bal.goods_stock_cap);
                        if matches!(h.from, Endpoint::Outside(_)) {
                            ledger.import_loads += 1;
                        }
                    }
                    Endpoint::Building(_) => {}
                    Endpoint::Outside(_) => {
                        ledger.exports += Cost::scaled(bal.export_price, f64::from(h.load));
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

    // The entry highways: `Outside` nodes with a road (not the railway's).
    let entries: Vec<(NodeId, Vec2)> = net
        .nodes()
        .filter(|(_, n)| {
            n.kind == NodeKind::Outside && !n.segments.is_empty() && !node_is_rail(&net, n)
        })
        .map(|(id, n)| (id, Vec2::new(n.pos.x, n.pos.z)))
        .collect();
    let nearest_entry = |p: Vec2| {
        entries
            .iter()
            .min_by(|a, b| {
                a.1.distance(p)
                    .total_cmp(&b.1.distance(p))
                    .then(a.0.cmp(&b.0))
            })
            .map(|&(id, _)| id)
    };

    let mine: Vec<BuildingId> = buildings
        .iter()
        .filter(|(id, b)| u64::from(id.index) % 64 == bucket && b.state == BuildingState::Active)
        .map(|(id, _)| id)
        .collect();
    for id in mine {
        let b = buildings.get(id).expect("listed");
        let zone = db.building(b.def).zone;
        match zone {
            ZoneType::Industrial => {
                let s = goods.stock.entry(id).or_default();
                *s = (*s + bal.goods_per_worker * filled(id)).min(bal.goods_stock_cap);
                if *s >= bal.goods_truck_load
                    && b.access().is_some()
                    && !goods
                        .hauls
                        .values()
                        .any(|h| h.from == Endpoint::Building(id))
                {
                    let to = match pick_shop(&buildings, &goods, db, id, bal.freight_radius_m) {
                        Some(shop) => Some((Endpoint::Building(shop), RunPoint::Building(shop))),
                        // Nobody to sell to nearby: the goods go out of the country.
                        None => nearest_entry(b.pos)
                            .map(|e| (Endpoint::Outside(e), RunPoint::Outside(e))),
                    };
                    if let Some((to, point)) = to {
                        let run = runs.start_run(
                            VehicleKind::Cargo,
                            RunPoint::Building(id),
                            point,
                            false,
                        );
                        *goods.stock.get_mut(&id).expect("just set") -= bal.goods_truck_load;
                        goods.hauls.insert(
                            run,
                            Haul {
                                from: Endpoint::Building(id),
                                to,
                                load: bal.goods_truck_load,
                            },
                        );
                    }
                }
            }
            ZoneType::ComLow | ZoneType::ComHigh => {
                let s = goods.stock.entry(id).or_default();
                *s = (*s - bal.goods_per_job * filled(id)).max(0.0);
                let empty = *s <= 0.0;
                let low = *s < bal.goods_import_threshold;
                let b = buildings.get(id).expect("listed");
                // No factory within reach, stock running out: a truck from the outside.
                if low
                    && b.access().is_some()
                    && !goods.hauls.values().any(|h| h.to == Endpoint::Building(id))
                    && !has_supplier(&buildings, db, id, bal.freight_radius_m)
                    && let Some(entry) = nearest_entry(b.pos)
                {
                    let run = runs.start_run(
                        VehicleKind::Cargo,
                        RunPoint::Outside(entry),
                        RunPoint::Building(id),
                        false,
                    );
                    goods.hauls.insert(
                        run,
                        Haul {
                            from: Endpoint::Outside(entry),
                            to: Endpoint::Building(id),
                            load: bal.goods_truck_load,
                        },
                    );
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

/// The shop with the least stock within `radius` of the factory `from` (smaller id on a tie).
fn pick_shop(
    buildings: &BuildingGrid,
    goods: &Goods,
    db: &cp_content::ContentDb,
    from: BuildingId,
    radius: f32,
) -> Option<BuildingId> {
    let origin = buildings.get(from)?.pos;
    let mut best: Option<(f32, BuildingId)> = None;
    for (id, b) in buildings.iter() {
        if b.state != BuildingState::Active
            || b.access().is_none()
            || !matches!(
                db.building(b.def).zone,
                ZoneType::ComLow | ZoneType::ComHigh
            )
            || b.pos.distance(origin) > radius
        {
            continue;
        }
        let s = goods.stock_of(id);
        if best.is_none_or(|(bs, _)| s < bs) {
            best = Some((s, id));
        }
    }
    best.map(|(_, id)| id)
}

/// Whether an active factory with a road stands within `radius` of the shop `shop`.
fn has_supplier(
    buildings: &BuildingGrid,
    db: &cp_content::ContentDb,
    shop: BuildingId,
    radius: f32,
) -> bool {
    let Some(origin) = buildings.get(shop).map(|b| b.pos) else {
        return false;
    };
    buildings.iter().any(|(_, b)| {
        b.state == BuildingState::Active
            && b.access().is_some()
            && db.building(b.def).zone == ZoneType::Industrial
            && b.pos.distance(origin) <= radius
    })
}

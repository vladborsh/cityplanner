//! Cargo hubs (T-216, §16 M5 (E)): a cargo station (`ServiceKind::Cargo`) whose platform is
//! joined by rail to the map's railway takes the exports of the city and brings its imports by
//! cargo train. Trucks bring the exports to the station, where they wait in the hub's stock for
//! the next train (`cp_industry::run_cargo_trains`, every `cargo_train_interval_h`); the
//! imports are ordered at the station and come with the train, which sends a truck on to the
//! building that wanted them. A hub is used only while it is joined to the railway and stands
//! ready, and only when it is nearer than the entry highway.
//!
//! The stock and the orders are serialized sim state ([`CargoHubs`]).

use std::collections::BTreeMap;

use bevy_ecs::resource::Resource;
use bevy_math::Vec2;
use cp_building::{BuildingGrid, BuildingId, BuildingState};
use cp_content::{CargoKind, ContentDb, ServiceKind};
use cp_core::Tick;
use cp_net::{NodeId, RailReach};
use serde::{Deserialize, Serialize};

use crate::goods::Endpoint;
use crate::model::TransitNet;
use crate::station::station_reaches_outside;

/// One cargo station: what waits for the next train.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hub {
    /// Exports that trucks brought, by cargo, in cargo order.
    pub out: Vec<(CargoKind, f32)>,
    /// Imports ordered for the next train: the cargo, the building that wanted it and the load.
    pub orders: Vec<(CargoKind, BuildingId, f32)>,
    /// The next train comes at this tick or later.
    pub next_at: Tick,
    /// Trains that have been.
    pub trains: u32,
}

impl Hub {
    /// Units of exports waiting.
    pub fn out_total(&self) -> f32 {
        self.out.iter().map(|&(_, a)| a).sum()
    }
}

/// The cargo stations' stock and orders. Serialized sim state.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CargoHubs {
    pub hubs: BTreeMap<BuildingId, Hub>,
}

impl CargoHubs {
    pub fn is_empty(&self) -> bool {
        self.hubs.is_empty()
    }

    /// A truck unloaded `load` of `cargo` at hub `b`.
    pub fn deposit(&mut self, b: BuildingId, cargo: CargoKind, load: f32, now: Tick, wait: u64) {
        let hub = self.hubs.entry(b).or_insert_with(|| Hub {
            next_at: Tick(now.0 + wait),
            ..Hub::default()
        });
        match hub.out.iter_mut().find(|(c, _)| *c == cargo) {
            Some((_, a)) => *a += load,
            None => {
                hub.out.push((cargo, load));
                hub.out.sort_by_key(|&(c, _)| c);
            }
        }
    }

    /// `to` ordered `load` of `cargo` from the outside through hub `b`.
    pub fn order(
        &mut self,
        b: BuildingId,
        cargo: CargoKind,
        to: BuildingId,
        load: f32,
        now: Tick,
        wait: u64,
    ) {
        let hub = self.hubs.entry(b).or_insert_with(|| Hub {
            next_at: Tick(now.0 + wait),
            ..Hub::default()
        });
        hub.orders.push((cargo, to, load));
    }

    /// Whether building `to` has an import of `cargo` waiting for a train.
    pub fn pending(&self, to: BuildingId, cargo: CargoKind) -> bool {
        self.hubs
            .values()
            .any(|h| h.orders.iter().any(|&(c, t, _)| c == cargo && t == to))
    }
}

/// Whether `def` is a cargo station (or port): a service building of kind `Cargo`.
pub fn is_hub_def(db: &ContentDb, def: cp_content::BuildingDefId) -> bool {
    db.building(def)
        .service
        .as_deref()
        .and_then(|s| db.service_id(s))
        .is_some_and(|s| db.service(s).kind == ServiceKind::Cargo)
}

/// The cargo stations that work: ready, with a road access, and (stations) joined by rail to the
/// map's railway. `(building, position)`, in building order.
pub fn hub_sites(
    db: &ContentDb,
    transit: &TransitNet,
    rail: &RailReach,
    buildings: &BuildingGrid,
) -> Vec<(BuildingId, Vec2)> {
    buildings
        .iter()
        .filter(|(id, b)| {
            b.state == BuildingState::Active
                && b.access().is_some()
                && is_hub_def(db, b.def)
                && station_reaches_outside(transit, rail, *id)
        })
        .map(|(id, b)| (id, b.pos))
        .collect()
}

/// Where the exports and imports of a building at `p` go through: the nearest of the entry
/// highways and the working hubs; a tie goes to the highway (the cheaper way: no train wait).
pub fn exit_for(
    entries: &[(NodeId, Vec2)],
    hubs: &[(BuildingId, Vec2)],
    p: Vec2,
) -> Option<Endpoint> {
    let entry = entries
        .iter()
        .min_by(|a, b| {
            a.1.distance(p)
                .total_cmp(&b.1.distance(p))
                .then(a.0.cmp(&b.0))
        })
        .map(|&(id, pos)| (pos.distance(p), id));
    let hub = hubs
        .iter()
        .min_by(|a, b| {
            a.1.distance(p)
                .total_cmp(&b.1.distance(p))
                .then(a.0.cmp(&b.0))
        })
        .map(|&(id, pos)| (pos.distance(p), id));
    match (entry, hub) {
        (Some((de, e)), Some((dh, h))) => Some(if dh < de {
            Endpoint::Building(h)
        } else {
            Endpoint::Outside(e)
        }),
        (Some((_, e)), None) => Some(Endpoint::Outside(e)),
        (None, Some((_, h))) => Some(Endpoint::Building(h)),
        (None, None) => None,
    }
}

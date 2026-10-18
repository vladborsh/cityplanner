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
    /// Trains (or ships) that have been.
    pub trains: u32,
    /// A cargo port: the ship that is sailing in, standing or sailing out (T-217).
    pub ship: Option<ShipVisit>,
    /// A cargo station: the train out on the railway (T-216b).
    pub train: Option<HubTrain>,
    /// Extra pay for the exported goods waiting here, in units of load at the export price: the
    /// unique factories' goods sell for more (T-215), and that follows them through the hub.
    pub premium: f32,
}

/// Where a ship is. Append-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShipPhase {
    /// Sailing in to the berth.
    Arriving,
    /// At the berth: the imports step off, the exports go on board.
    Docked,
    /// Sailing out; the record goes when it is gone.
    Leaving,
}

/// One visit of a ship to a cargo port: the phase it is in and when that phase ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShipVisit {
    pub phase: ShipPhase,
    pub until: Tick,
}

/// A cargo train of a station: the run on the railway and where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubTrain {
    pub run: cp_traffic::RunId,
    pub phase: crate::intercity::TrainPhase,
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
    pub fn deposit(
        &mut self,
        b: BuildingId,
        cargo: CargoKind,
        load: f32,
        premium: f32,
        now: Tick,
        wait: u64,
    ) {
        let hub = self.hubs.entry(b).or_insert_with(|| Hub {
            next_at: Tick(now.0 + wait),
            ..Hub::default()
        });
        hub.premium += premium;
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

/// Whether `def` is a cargo port: a cargo hub on the shore, reached by ship instead of rail.
pub fn is_port_def(db: &ContentDb, def: cp_content::BuildingDefId) -> bool {
    db.building(def)
        .service
        .as_deref()
        .and_then(|s| db.service_id(s))
        .is_some_and(|s| {
            let s = db.service(s);
            s.kind == ServiceKind::Cargo && s.site == cp_content::ServiceSite::Shore
        })
}

/// The cargo stations that work: ready, with a road access, and (stations) joined by rail to the
/// map's railway; a port needs no rail. `(building, position)`, in building order.
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
                && (is_port_def(db, b.def) || station_reaches_outside(transit, rail, *id))
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

/// How far out a ship sails from its berth, metres (the client draws it that far off).
pub const SAIL_M: f32 = 300.0;

/// A ship at a cargo port, as the client draws it (T-217): between sea and berth along `dir`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShipPose {
    pub port: BuildingId,
    /// The berth: the port building's position.
    pub berth: Vec2,
    /// Unit vector from the berth out over the water.
    pub dir: Vec2,
    pub phase: ShipPhase,
    /// How far through the phase it is now and a tick later (`0..=1`).
    pub t: f32,
    pub next_t: f32,
}

impl ShipPose {
    fn at(&self, t: f32) -> Vec2 {
        match self.phase {
            ShipPhase::Arriving => self.berth + self.dir * SAIL_M * (1.0 - t),
            ShipPhase::Docked => self.berth,
            ShipPhase::Leaving => self.berth + self.dir * SAIL_M * t,
        }
    }

    /// Where the ship is now, and one tick later (the client draws between the two).
    pub fn pos(&self) -> Vec2 {
        self.at(self.t)
    }

    pub fn next_pos(&self) -> Vec2 {
        self.at(self.next_t)
    }
}

/// The direction from `berth` along which the water goes on the longest (`wet` tells water).
pub fn water_dir(berth: Vec2, rot: f32, wet: impl Fn(Vec2) -> bool) -> Vec2 {
    let step = cp_content::ServiceSite::SHORE_REACH_M;
    let mut best = (0, Vec2::from_angle(rot));
    for k in 0..16 {
        let d = Vec2::from_angle(k as f32 * std::f32::consts::TAU / 16.0);
        let run = (1..=(SAIL_M / step) as u32)
            .take_while(|&i| wet(berth + d * step * i as f32))
            .count();
        if run > best.0 {
            best = (run, d);
        }
    }
    best.1
}

/// The ships at the ports now, in building order. `sail` and `dwell` are the phases' lengths in
/// ticks.
pub fn ship_poses(
    hubs: &CargoHubs,
    buildings: &BuildingGrid,
    now: Tick,
    sail: u64,
    dwell: u64,
    wet: impl Fn(Vec2) -> bool,
) -> Vec<ShipPose> {
    hubs.hubs
        .iter()
        .filter_map(|(&id, h)| {
            let v = h.ship?;
            let b = buildings.get(id)?;
            let len = if v.phase == ShipPhase::Docked {
                dwell
            } else {
                sail
            }
            .max(1) as f32;
            let left = v.until.0.saturating_sub(now.0) as f32;
            let t = |left: f32| 1.0 - (left / len).clamp(0.0, 1.0);
            Some(ShipPose {
                port: id,
                berth: b.pos,
                dir: water_dir(b.pos, b.rot, &wet),
                phase: v.phase,
                t: t(left),
                next_t: t((left - 1.0).max(0.0)),
            })
        })
        .collect()
}

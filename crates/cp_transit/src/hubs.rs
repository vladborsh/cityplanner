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

/// How far a ship sails from its berth along the water, metres.
pub const SAIL_M: f32 = 300.0;
/// Metres between the points of a ship's line.
const ROUTE_STEP: f32 = 8.0;

/// A ship at a cargo port, as the client draws it (T-217): between sea and berth along the
/// water (BUG-071: the line follows the river's centre, it does not run straight onto land).
#[derive(Clone, Debug, PartialEq)]
pub struct ShipPose {
    pub port: BuildingId,
    /// The berth: the port building's position.
    pub berth: Vec2,
    /// Unit vector from the berth out over the water (to the first wet point).
    pub dir: Vec2,
    /// Unit vector along the water at the berth, the way a ship lies alongside the bank
    /// (square to `dir`, pointing the way the line goes on).
    pub heading: Vec2,
    /// The line: the berth, the bank, then the river's centre line out for up to
    /// [`SAIL_M`] metres (it ends where the water ends). Never empty.
    pub points: Vec<Vec2>,
    pub phase: ShipPhase,
    /// How far through the phase it is now and a tick later (`0..=1`).
    pub t: f32,
    pub next_t: f32,
}

impl ShipPose {
    /// Length of the line, metres.
    pub fn length(&self) -> f32 {
        self.points.windows(2).map(|w| w[0].distance(w[1])).sum()
    }

    /// The point `s` metres along the line from the berth (clamped to it).
    pub fn along(&self, s: f32) -> Vec2 {
        let mut left = s.max(0.0);
        for w in self.points.windows(2) {
            let d = w[0].distance(w[1]);
            if left <= d {
                return w[0].lerp(w[1], if d > 0.0 { left / d } else { 0.0 });
            }
            left -= d;
        }
        self.points.last().copied().unwrap_or(self.berth)
    }

    fn at(&self, t: f32) -> Vec2 {
        let len = self.length();
        match self.phase {
            ShipPhase::Arriving => self.along(len * (1.0 - t)),
            ShipPhase::Docked => self.berth,
            ShipPhase::Leaving => self.along(len * t),
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

/// How far the water goes from `p` along `d`, up to `max` metres, in 4 m steps.
fn wet_run(p: Vec2, d: Vec2, max: f32, wet: &impl Fn(Vec2) -> bool) -> f32 {
    let mut r = 0.0;
    while r + 4.0 <= max && wet(p + d * (r + 4.0)) {
        r += 4.0;
    }
    r
}

/// The line of the river's centre from `start` (a wet point) heading `dir`: steps of 8 m that
/// keep to the water, turn to follow a bend (the heading with the longest water ahead) and are
/// pulled to the middle of the water across; ends after [`SAIL_M`] metres or where the water ends.
fn river_line(start: Vec2, dir: Vec2, wet: &impl Fn(Vec2) -> bool) -> Vec<Vec2> {
    let mut pts = vec![start];
    let (mut p, mut d) = (start, dir);
    let mut walked = 0.0;
    while walked < SAIL_M {
        let mut best: Option<(f32, i32, Vec2)> = None;
        for k in [0i32, 1, -1, 2, -2, 3, -3, 4, -4] {
            let h = Vec2::from_angle(k as f32 * 12f32.to_radians()).rotate(d);
            let q = p + h * ROUTE_STEP;
            if !wet(q) {
                continue;
            }
            let ahead = wet_run(q, h, 5.0 * ROUTE_STEP, wet);
            if best.is_none_or(|(a, bk, _)| ahead > a || (ahead == a && k.abs() < bk.abs())) {
                best = Some((ahead, k, h));
            }
        }
        let Some((_, _, h)) = best else { break };
        let mut q = p + h * ROUTE_STEP;
        let side = Vec2::new(-h.y, h.x);
        let left = wet_run(q, side, 40.0, wet);
        let right = wet_run(q, -side, 40.0, wet);
        let moved = q + side * ((left - right) / 2.0);
        if wet(moved) {
            q = moved;
        }
        pts.push(q);
        d = (q - p).try_normalize().unwrap_or(h);
        p = q;
        walked += ROUTE_STEP;
    }
    pts
}

/// The water route of a ship at `berth` (BUG-071): `(dir, heading, points)`, see [`ShipPose`].
/// The bank is the nearest wet point round the berth; from there the longer of the ways along
/// the river (up- or downstream) is taken. Without water the line is the berth alone.
pub fn ship_route(berth: Vec2, rot: f32, wet: impl Fn(Vec2) -> bool) -> (Vec2, Vec2, Vec<Vec2>) {
    // The lot is 4x4 cells: its centre can be a lot farther from the water than its cells.
    let reach = cp_content::ServiceSite::SHORE_REACH_M + 24.0;
    let mut bank: Option<(f32, Vec2)> = None;
    for k in 0..16 {
        let d = Vec2::from_angle(k as f32 * std::f32::consts::TAU / 16.0);
        let mut r = 2.0;
        while r <= reach + 0.5 {
            if wet(berth + d * r) {
                if bank.is_none_or(|(br, _)| r < br) {
                    bank = Some((r, d));
                }
                break;
            }
            r += 2.0;
        }
    }
    let dir0 = Vec2::from_angle(rot);
    let Some((r, d)) = bank else {
        return (dir0, Vec2::new(-dir0.y, dir0.x), vec![berth]);
    };
    // A few metres into the water, so the hull's centre is wet.
    let entry = berth + d * r;
    let dock = if wet(entry + d * 10.0) {
        entry + d * 10.0
    } else {
        entry
    };
    let side = Vec2::new(-d.y, d.x);
    let mut best: Vec<Vec2> = Vec::new();
    for k in 0..8 {
        let along = Vec2::from_angle(k as f32 * std::f32::consts::TAU / 8.0).rotate(side);
        let line = river_line(dock, along, &wet);
        if line.len() > best.len() {
            best = line;
        }
    }
    let mut points = vec![berth];
    if dock.distance(berth) > 0.5 {
        points.push(dock);
    }
    points.extend(best.into_iter().skip(1));
    let heading = points
        .get(points.len().min(6) - 1)
        .map(|&p| (p - dock).try_normalize())
        .unwrap_or(None)
        .unwrap_or(side);
    (d, heading, points)
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
            let (dir, heading, points) = ship_route(b.pos, b.rot, &wet);
            let left = v.until.0.saturating_sub(now.0) as f32;
            let t = |left: f32| 1.0 - (left / len).clamp(0.0, 1.0);
            Some(ShipPose {
                port: id,
                berth: b.pos,
                dir,
                heading,
                points,
                phase: v.phase,
                t: t(left),
                next_t: t((left - 1.0).max(0.0)),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A winding river: water within 30 m of z = 150 sin(x / 200), x in 0..2000.
    fn river(p: Vec2) -> bool {
        (0.0..2000.0).contains(&p.x) && (p.y - 150.0 * (p.x / 200.0).sin()).abs() < 30.0
    }

    #[test]
    fn the_route_follows_a_winding_river_and_stays_on_the_water() {
        // A berth on the north bank at x = 500, 40 m from the centre line.
        let berth = Vec2::new(500.0, 150.0 * (2.5f32).sin() + 40.0);
        let (dir, heading, points) = ship_route(berth, 0.0, river);
        assert!(points.len() > 20, "{} points", points.len());
        assert!(
            dir.dot(Vec2::NEG_Y) > 0.5,
            "the water is to the south: {dir:?}"
        );
        assert!(heading.dot(dir).abs() < 0.9, "alongside, not into the bank");
        // Every point after the berth is on the water.
        assert!(points.iter().skip(1).all(|&p| river(p)), "{points:?}");
        // The steps are short: a straight 300 m line would have left the river long ago.
        assert!(points.windows(2).all(|w| w[0].distance(w[1]) < 30.0));
        let end = *points.last().unwrap();
        let line = (end - points[1]).length();
        assert!(line > 100.0, "it goes a long way: {line}");
        // Deterministic.
        assert_eq!(ship_route(berth, 0.0, river).2, points);
    }

    #[test]
    fn a_pose_walks_its_line_and_a_berth_without_water_has_none() {
        let pose = ShipPose {
            port: BuildingId::new(0, 0),
            berth: Vec2::ZERO,
            dir: Vec2::X,
            heading: Vec2::Y,
            points: vec![Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(10.0, 30.0)],
            phase: ShipPhase::Leaving,
            t: 0.5,
            next_t: 0.75,
        };
        assert_eq!(pose.length(), 40.0);
        assert_eq!(pose.pos(), Vec2::new(10.0, 10.0));
        assert_eq!(pose.next_pos(), Vec2::new(10.0, 20.0));
        let (_, _, dry) = ship_route(Vec2::new(5000.0, 5000.0), 0.3, |_| false);
        assert_eq!(dry, vec![Vec2::new(5000.0, 5000.0)]);
    }
}

//! What bulldozing gives back (BUG-097): [`demolish_refund`](cp_content::Balance::demolish_refund)
//! of what the player paid for the thing removed, at today's prices. The command pays it into the
//! treasury (`command::apply`), the bulldozer shows it before the click; both call
//! [`demolition_refund`], so the "+N" on the ghost is the money that comes back.
//!
//! What counts as paid:
//! * a building placed by the player (a service, a unique building, a park, a station): its
//!   `cost`. A grown building (houses, shops, industry of a zone) gives nothing;
//! * road: the length removed at the price of the segment's road and elevation now
//!   ([`cp_net::removal_value`]; an upgrade counts at the road it upgraded to). The roads the map
//!   came with (the entry highway, its stub, the entry railway) were never paid: nothing;
//! * a pipe or power line link: its length at the utility's price;
//! * a bus stop: its price. A station's stop goes with its building.

use bevy_math::Vec2;
use cp_building::{BuildingCommand, BuildingGrid, BuildingId};
use cp_content::{ContentDb, UtilityKind};
use cp_core::Cost;
use cp_net::{NetCommand, NetGraph, SegmentId};
use cp_terrain::MapMeta;
use cp_transit::{StopKind, TransitCommand, TransitNet};
use cp_utility::{UtilityCommand, UtilityGraph, UtilityNet};

use crate::command::SimCommand;

/// A segment lying this close (metres, at its ends and its middle) to the map's entry road or
/// railway is one the map built.
const MAP_ROAD_TOL: f32 = 4.0;

/// What [`demolition_refund`] reads: the sim's state, or a view of it.
#[derive(Clone, Copy)]
pub struct RefundInputs<'a> {
    pub content: &'a ContentDb,
    pub net: &'a NetGraph,
    pub buildings: &'a BuildingGrid,
    pub utilities: &'a UtilityGraph,
    pub transit: Option<&'a TransitNet>,
    /// The map's entry road and railway; `None`: no map, every road was paid for.
    pub meta: Option<&'a MapMeta>,
}

impl<'a> RefundInputs<'a> {
    /// The inputs as a view of the sim shows them (the bulldozer's "+N"); `None` without content.
    pub fn of_view(view: &'a crate::SimView) -> Option<Self> {
        Some(Self {
            content: view.content.as_deref()?,
            net: &view.net,
            buildings: &view.buildings,
            utilities: &view.utilities,
            transit: Some(&view.transit.net),
            meta: view.terrain.as_deref().map(|t| t.meta()),
        })
    }
}

/// The share of `paid` that comes back, rounded once to the cent, half away from zero (as
/// [`Cost::scaled`] does).
fn share(i: &RefundInputs, paid: Cost) -> Cost {
    let k = f64::from(i.content.balance().demolish_refund.clamp(0.0, 1.0));
    Cost((paid.0 as f64 * k).round() as i64)
}

/// What bulldozing `cmd` gives back, before it runs. Zero for every command that removes
/// nothing the player paid for. A road command over buildings (BUG-096) is not counted here:
/// see [`building_refund`].
pub fn demolition_refund(i: &RefundInputs, cmd: &SimCommand) -> Cost {
    match cmd {
        SimCommand::Building(BuildingCommand::Demolish { building }) => {
            building_refund(i, *building)
        }
        SimCommand::Net(c) | SimCommand::NetDemolish(c) => match c {
            NetCommand::RemoveSegment { seg } => road_refund(i, *seg, None),
            NetCommand::RemovePart { seg, s0, s1 } => road_refund(i, *seg, Some((*s0, *s1))),
            _ => Cost::ZERO,
        },
        SimCommand::Utility(UtilityCommand::RemoveLink { link }) => {
            let Some(l) = i.utilities.link(*link) else {
                return Cost::ZERO;
            };
            let kind = match l.net {
                UtilityNet::Power => UtilityKind::Power,
                UtilityNet::Pipe => UtilityKind::Water,
            };
            i.content
                .utility_for_kind(kind)
                .map_or(Cost::ZERO, |(_, def)| {
                    share(i, cp_net::segment_cost(l.length, def.cost_per_m))
                })
        }
        SimCommand::Transit(TransitCommand::RemoveStop { stop }) => {
            match i.transit.and_then(|t| t.stops.get(*stop)).map(|s| s.kind) {
                Some(StopKind::Road { .. }) => {
                    share(i, Cost::from_units(i.content.balance().bus_stop_cost))
                }
                _ => Cost::ZERO,
            }
        }
        _ => Cost::ZERO,
    }
}

/// What demolishing `building` gives back: a share of its price if the player placed it, nothing
/// for a grown one (or an unknown id).
pub fn building_refund(i: &RefundInputs, building: BuildingId) -> Cost {
    let Some(b) = i.buildings.get(building) else {
        return Cost::ZERO;
    };
    let def = i.content.building(b.def);
    if cp_building::is_growable(def) {
        return Cost::ZERO;
    }
    share(i, Cost::from_units(def.cost))
}

fn road_refund(i: &RefundInputs, seg: SegmentId, part: Option<(f32, f32)>) -> Cost {
    if is_map_road(i, seg) {
        return Cost::ZERO;
    }
    cp_net::removal_value(i.net, i.content, seg, part).map_or(Cost::ZERO, |v| share(i, v))
}

/// Whether `seg` is a piece of the road or railway the map came with: its ends and middle lie
/// on the entry geometry of the map.
pub fn is_map_road(i: &RefundInputs, seg: SegmentId) -> bool {
    let (Some(meta), Some(s)) = (i.meta, i.net.segment(seg)) else {
        return false;
    };
    let mut lines: Vec<Vec<Vec2>> = Vec::new();
    if let Some(e) = &meta.entry_road {
        let (inbound, outbound) = e.carriageways();
        lines.push(inbound);
        lines.push(outbound);
        lines.push(vec![e.junction(), e.stub_end()]);
    }
    if let Some(e) = &meta.entry_rail {
        lines.push(e.points());
    }
    let on = |p: Vec2| {
        lines.iter().any(|l| {
            l.windows(2)
                .any(|w| distance_to_segment(p, w[0], w[1]) <= MAP_ROAD_TOL)
        })
    };
    [0.0, 0.5, 1.0].iter().all(|&t| {
        let q = s.curve.point(t);
        on(Vec2::new(q.x, q.z))
    })
}

fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 {
        ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

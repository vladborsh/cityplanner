//! World poses of vehicles and pedestrians for clients (render). Read-only: a snapshot is
//! built from `Agents` after a tick; interpolation between two snapshots is the client's job.

use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingGrid, BuildingId};
use cp_citizen::TripBook;
use cp_net::{JunctionGeometry, LaneConnection, LaneId, NetGraph};

use crate::agents::{Agents, Pedestrian, Vehicle, access_of};
use crate::graph::{LaneNode, PathGraph};

/// Stable key of an agent: arena `index << 32 | generation`. A frame lists its agents in arena
/// order, which is ascending by key: clients match two frames by merging them (BUG-104: with the
/// generation in the high bits a reused slot broke the order, and those cars were drawn without
/// interpolation, jumping from tick to tick).
pub type AgentKey = u64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehiclePose {
    pub key: AgentKey,
    /// `cp_content::VehicleKind` as a number.
    pub kind: u8,
    /// Centre of the vehicle.
    pub pos: Vec3,
    /// Heading in radians: `atan2(dir.x, dir.z)`.
    pub yaw: f32,
    /// Metres per second.
    pub speed: f32,
    /// Colour the vehicle is drawn in, when it has one of its own: the colour of the line of a
    /// bus (`cp_sim` fills it in, T-163). `None` = the kind's colour.
    pub tint: Option<[u8; 3]>,
}

/// An agent near the building it set off from or is heading into (§25.2): the client draws it
/// on its way between the building's portal and the sim position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Portal {
    pub building: BuildingId,
    /// `0` at the portal, `1` where the drawn agent meets the sim position.
    pub progress: f32,
    /// Leaving the building (`progress` grows), else entering it (`progress` grows too).
    pub leaving: bool,
}

/// Metres of the walk at either end of a route that a pedestrian spends on the way between
/// the door and the sidewalk (§25.2).
pub const DOOR_WALK_M: f32 = 10.0;

/// Free half-width a pedestrian may stray from its lane's centre line on a crossing or corner,
/// metres (§25.2).
pub const CROSSING_ROOM_M: f32 = 0.3;

/// Width of a sidewalk kept clear of the drawn spread, metres: half a body at either edge.
const SIDEWALK_MARGIN_M: f32 = 1.2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PedestrianPose {
    pub key: AgentKey,
    /// Position on the ground plane (x, z).
    pub pos: Vec2,
    /// The door it walks out of or into, near either end of its route.
    pub portal: Option<Portal>,
    /// How far either side of the lane's centre line it may be drawn, metres.
    pub room: f32,
}

/// Poses of all live agents after one tick, each list ascending by `key`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentFrame {
    pub vehicles: Vec<VehiclePose>,
    pub pedestrians: Vec<PedestrianPose>,
}

/// The key of agent `index` / `generation` in an [`AgentFrame`].
pub fn agent_key(index: u32, generation: u32) -> AgentKey {
    u64::from(index) << 32 | u64::from(generation)
}

fn yaw_of(d: Vec3) -> f32 {
    d.x.atan2(d.z)
}

/// Where a vehicle's axles sit, as shares of its length behind the front bumper: the body is
/// drawn on the line from the rear axle to the front axle, both on the path the front drove.
pub const FRONT_AXLE: f32 = 0.2;
pub const REAR_AXLE: f32 = 0.8;

/// Pose `(centre, yaw)` of a vehicle `length` metres long, or `None` for one not yet on the road
/// / off the network. Points on the path are found half a length and more behind the front
/// bumper, back through the junction connection and the lane before it (BUG-104: clamped to the
/// start of the current piece the body stood still for half a length at every junction and then
/// jumped ahead). The heading is the line from the rear axle to the front axle, a two-axle model
/// (BUG-105): the rear follows the front into a turn and cuts its inside a little, so a long
/// truck sweeps round a corner instead of turning on the spot.
pub(crate) fn vehicle_pose(
    v: &Vehicle,
    length: f32,
    graph: &PathGraph,
    net: &NetGraph,
    geo: &JunctionGeometry,
) -> Option<(Vec3, f32)> {
    if !v.spawned {
        return None;
    }
    let (front, dir) = point_behind(v, length * FRONT_AXLE, graph, net, geo)?;
    let (rear, _) = point_behind(v, length * REAR_AXLE, graph, net, geo)?;
    let axis = front - rear;
    // On the first metres of its first lane both axles are clamped to the lane start.
    let axis = if Vec2::new(axis.x, axis.z).length() > length * 0.25 {
        axis.normalize()
    } else {
        return Some((
            point_behind(v, length * 0.5, graph, net, geo)?.0,
            yaw_of(dir),
        ));
    };
    let centre = rear + axis * length * (0.5 - (1.0 - REAR_AXLE));
    Some((centre, yaw_of(axis)))
}

/// The connection `from → to` of the junction `from` runs into.
fn connection<'a>(
    from: LaneId,
    to: LaneId,
    graph: &PathGraph,
    net: &'a NetGraph,
    geo: &'a JunctionGeometry,
) -> Option<&'a LaneConnection> {
    let seg = net.segment(net.lane(from)?.segment)?;
    let node = if graph.lane_forward(from)? {
        seg.end
    } else {
        seg.start
    };
    geo.get(node)?
        .links
        .connections
        .iter()
        .find(|c| c.from == from && c.to == to)
}

/// Point `s` metres along a connection curve, by arc length (the curve's own parameter runs
/// faster on the straighter end), and the direction there.
fn connection_point(c: &LaneConnection, s: f32) -> (Vec3, Vec3) {
    if c.length <= 1e-3 {
        return (c.curve.p3, c.curve.tangent(1.0));
    }
    const CELLS: usize = 16;
    let target = s.clamp(0.0, c.length) / c.length;
    let mut chords = [0.0f32; CELLS];
    let mut prev = c.curve.p0;
    let mut total = 0.0;
    for (i, chord) in chords.iter_mut().enumerate() {
        let p = c.curve.point((i + 1) as f32 / CELLS as f32);
        *chord = prev.distance(p);
        total += *chord;
        prev = p;
    }
    let mut left = target * total;
    let mut t = 1.0;
    for (i, chord) in chords.iter().enumerate() {
        if left <= *chord {
            t = (i as f32 + if *chord > 1e-6 { left / chord } else { 0.0 }) / CELLS as f32;
            break;
        }
        left -= chord;
    }
    (c.curve.point(t), c.curve.tangent(t))
}

/// Point on a lane `s` metres along it, with the lane's direction there.
fn lane_point(lane: &LaneNode, s: f32) -> (Vec3, Vec3) {
    let s = s.max(0.0);
    let ahead = lane.point3_at(s + 1.0);
    let behind = lane.point3_at((s - 1.0).max(0.0));
    (lane.point3_at(s), ahead - behind)
}

/// The point `back` metres behind the front bumper along the way the vehicle came, and the
/// path's direction there. A car on the first lane of its path is placed on that lane's line.
pub(crate) fn point_behind(
    v: &Vehicle,
    back: f32,
    graph: &PathGraph,
    net: &NetGraph,
    geo: &JunctionGeometry,
) -> Option<(Vec3, Vec3)> {
    let mut hop = usize::from(v.hop);
    let mut in_conn = v.in_conn;
    let mut s = v.s;
    let mut left = back.max(0.0);
    // A few pieces back at most: a lane between two junctions is never that short.
    for _ in 0..6 {
        if in_conn {
            let (from, to) = v.path.steps.get(hop).copied()?;
            let c = connection(from, to, graph, net, geo)?;
            if s >= left {
                return Some(connection_point(c, s - left));
            }
            left -= s.max(0.0);
            in_conn = false;
            s = graph.car_lane(from)?.s_out;
        } else {
            let id = if hop == 0 {
                v.path.start
            } else {
                v.path.steps.get(hop - 1)?.1
            };
            let lane = graph.car_lane(id)?;
            if hop == 0 || s - left >= lane.s_in {
                return Some(lane_point(lane, s - left));
            }
            left -= (s - lane.s_in).max(0.0);
            hop -= 1;
            in_conn = true;
            let (from, to) = v.path.steps.get(hop).copied()?;
            s = connection(from, to, graph, net, geo)?.length;
        }
    }
    let lane = graph.car_lane(v.lane())?;
    Some(lane_point(lane, v.s - back))
}

/// Whether the walking route end `(lane, s)` is the sidewalk point of building `b`'s access.
fn at_access(
    b: BuildingId,
    lane: LaneId,
    s: f32,
    graph: &PathGraph,
    net: &NetGraph,
    buildings: &BuildingGrid,
) -> bool {
    buildings
        .get(b)
        .and_then(|b| access_of(net, b))
        .and_then(|a| graph.resolve_ped(&a))
        .and_then(|(slot, at)| Some((graph.ped.get(slot as usize)?.as_ref()?.id, at)))
        .is_some_and(|(l, at)| l == lane && (at - s).abs() < 1e-2)
}

/// The door portal of a pedestrian within [`DOOR_WALK_M`] of the start of its first leg or the
/// end of its last, when that route end is the access of its trip's building (a walk to or
/// from a stop has no door at the stop end).
fn ped_portal(
    p: &Pedestrian,
    trips: &TripBook,
    graph: &PathGraph,
    net: &NetGraph,
    buildings: &BuildingGrid,
) -> Option<Portal> {
    if !p.started {
        return None;
    }
    let n = p.route.states.len();
    let first = p.leg == 0;
    let last = p.leg as usize == n;
    if !first && !last {
        return None;
    }
    // A leg that is both first and last splits between its two doors; a short end leg ends
    // its door walk where it ends.
    let window = if first && last {
        DOOR_WALK_M.min(p.leg_len * 0.5)
    } else {
        DOOR_WALK_M.min(p.leg_len)
    };
    if window <= 1e-3 {
        return None;
    }
    let left = p.leg_len - p.s;
    let trip = trips.get(p.trip)?;
    if first
        && p.s < window
        && (!last || p.s <= left)
        && at_access(
            trip.from,
            p.route.start,
            p.route.start_s,
            graph,
            net,
            buildings,
        )
    {
        return Some(Portal {
            building: trip.from,
            progress: p.s / window,
            leaving: true,
        });
    }
    if last
        && left < window
        && at_access(trip.to, p.route.goal, p.route.goal_s, graph, net, buildings)
    {
        return Some(Portal {
            building: trip.to,
            progress: 1.0 - left / window,
            leaving: false,
        });
    }
    None
}

/// Half the free width of the pedestrian's current leg: of its sidewalk less a margin, or
/// [`CROSSING_ROOM_M`] on a crossing or corner.
fn ped_room(p: &Pedestrian, net: &NetGraph) -> f32 {
    let (i, states) = (p.leg as usize, &p.route.states);
    let lane = if i == 0 {
        Some(p.route.start)
    } else if i >= states.len() {
        Some(p.route.goal)
    } else {
        (states[i - 1].0 == states[i].0).then_some(states[i].0)
    };
    lane.and_then(|l| net.lane(l)).map_or(CROSSING_ROOM_M, |l| {
        ((l.width - SIDEWALK_MARGIN_M) * 0.5).max(0.0)
    })
}

impl Agents {
    /// Poses of every agent that is on the road / walking. `trips` and `buildings` give the
    /// portals of agents near their building (§25.2).
    pub fn frame(
        &self,
        graph: &PathGraph,
        net: &NetGraph,
        geo: &JunctionGeometry,
        trips: &TripBook,
        buildings: &BuildingGrid,
    ) -> AgentFrame {
        let vehicles = self
            .vehicles()
            .filter_map(|(id, v)| {
                let (pos, yaw) = vehicle_pose(v, self.specs().of(v).length, graph, net, geo)?;
                Some(VehiclePose {
                    key: agent_key(id.index, id.generation),
                    kind: v.kind_id(),
                    pos,
                    yaw,
                    speed: v.speed,
                    tint: None,
                })
            })
            .collect();
        let pedestrians = self
            .pedestrians()
            // A pedestrian between legs or waiting at a red crossing stays in the frame (it
            // stands where it is); only one not yet on its first leg has no position (BUG-064).
            .filter(|(_, p)| p.started || p.leg > 0)
            .map(|(id, p)| PedestrianPose {
                key: agent_key(id.index, id.generation),
                pos: p.pos,
                portal: ped_portal(p, trips, graph, net, buildings),
                room: ped_room(p, net),
            })
            .collect();
        AgentFrame {
            vehicles,
            pedestrians,
        }
    }
}

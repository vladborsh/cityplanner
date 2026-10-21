//! World poses of vehicles and pedestrians for clients (render). Read-only: a snapshot is
//! built from `Agents` after a tick; interpolation between two snapshots is the client's job.

use bevy_math::{Vec2, Vec3};
use cp_net::{JunctionGeometry, LaneConnection, LaneId, NetGraph};

use crate::agents::{Agents, Vehicle};
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PedestrianPose {
    pub key: AgentKey,
    /// Position on the ground plane (x, z).
    pub pos: Vec2,
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

impl Agents {
    /// Poses of every agent that is on the road / walking.
    pub fn frame(&self, graph: &PathGraph, net: &NetGraph, geo: &JunctionGeometry) -> AgentFrame {
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
            })
            .collect();
        AgentFrame {
            vehicles,
            pedestrians,
        }
    }
}

//! World poses of vehicles and pedestrians for clients (render). Read-only: a snapshot is
//! built from `Agents` after a tick; interpolation between two snapshots is the client's job.

use bevy_math::{Vec2, Vec3};
use cp_core::curve::CubicBezier3;
use cp_net::{JunctionGeometry, NetGraph};

use crate::agents::{Agents, Vehicle};
use crate::graph::PathGraph;

/// Stable key of an agent: arena `(index, generation)`. Ascending by index within a frame.
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
    u64::from(generation) << 32 | u64::from(index)
}

fn yaw_of(d: Vec3) -> f32 {
    d.x.atan2(d.z)
}

/// Pose `(centre, yaw)` of a vehicle, or `None` for one not yet on the road / off the network.
pub(crate) fn vehicle_pose(
    v: &Vehicle,
    half_length: f32,
    graph: &PathGraph,
    net: &NetGraph,
    geo: &JunctionGeometry,
) -> Option<(Vec3, f32)> {
    if !v.spawned {
        return None;
    }
    if v.in_conn {
        let (from, to) = v.path.steps.get(usize::from(v.hop)).copied()?;
        let lane = net.lane(from)?;
        let seg = net.segment(lane.segment)?;
        let node = if graph.lane_forward(from)? {
            seg.end
        } else {
            seg.start
        };
        let c = geo
            .get(node)?
            .links
            .connections
            .iter()
            .find(|c| c.from == from && c.to == to)?;
        let curve: &CubicBezier3 = &c.curve;
        let t = if c.length > 1e-3 {
            ((v.s - half_length) / c.length).clamp(0.0, 1.0)
        } else {
            1.0
        };
        return Some((curve.point(t), yaw_of(curve.tangent(t))));
    }
    let lane = graph.car_lane(v.lane())?;
    let s = (v.s - half_length).max(0.0);
    let p = lane.point3_at(s);
    let ahead = lane.point3_at(s + 1.0);
    let behind = lane.point3_at((s - 1.0).max(0.0));
    Some((p, yaw_of(ahead - behind)))
}

impl Agents {
    /// Poses of every agent that is on the road / walking.
    pub fn frame(&self, graph: &PathGraph, net: &NetGraph, geo: &JunctionGeometry) -> AgentFrame {
        let vehicles = self
            .vehicles()
            .filter_map(|(id, v)| {
                let (pos, yaw) = vehicle_pose(v, self.specs().of(v).length * 0.5, graph, net, geo)?;
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
            .filter(|(_, p)| p.started)
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

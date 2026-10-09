//! Junction control (`system-design.md` §6.5): priority and traffic lights.
//!
//! **Saved state** is only [`Node::control`](crate::model::Node): the
//! [`JunctionControl`] enum (`Priority`, then the appended `TrafficLight`,
//! `ManualPriority`, `ManualTrafficLight`; appending enum variants needs no save version
//! bump, §4). Everything here — priority, phases, signal timing — is **derived**, a pure
//! function of the graph, the control value and the content ranks: it lives in
//! [`Junction::control`](crate::junction::Junction), is never saved and never hashed, and
//! is rebuilt by the `Network` phase like the rest of the junction.
//!
//! **Auto-install** ([`auto_light_rule`]): a node gets a light when at least 3 segments
//! meet and at least 2 of them have 4 or more vehicle (`Car`/`Bus`) lanes. The rule is
//! applied whenever the junction is recomputed *unless the player overrode it*: the
//! `Priority`/`TrafficLight` pair is "automatic" (the rule decides), `ManualPriority` /
//! `ManualTrafficLight` are the player's choice and are never touched by the rule. The
//! `ToggleTrafficLight` command flips between the two manual states.
//!
//! **Priority**: the *main road* is the pair of arms with the highest `RoadDef.rank`
//! (ties: the wider road, then the lower `SegmentId`). A connection has [`Priority::Main`]
//! if it starts on a main arm, except a left turn that conflicts with a straight
//! connection of the other main arm (it must yield to oncoming traffic). Every other
//! connection yields to the connections it conflicts with ([`Links::conflicts`]); the
//! traffic code uses `priority` plus the conflict list. Nodes with one or two arms have no
//! yielding. With a light, signals decide and `priority` is ignored.
//!
//! **Phases**: greedy colouring of the conflict graph, connections in index order, each
//! taking the lowest phase none of its already-coloured conflicts has — so no two
//! conflicting connections share a phase and every connection is in exactly one. A
//! crossing is walkable in a phase when no connection of that phase starts or ends on its
//! arm (pedestrians cross the arms the straight-through group does not use). If some
//! crossing is walkable in no phase, a final pedestrian-only phase opens all of them.
//!
//! **Timing** is in game time: each phase shows green for [`GREEN_TICKS`] (20 s) then
//! yellow for [`YELLOW_TICKS`] (3 s); 1 tick = 50 ms. The signal is a **pure function of
//! `(plan, tick)`**: the cycle position is `(tick + offset) mod cycle`, where `offset` is a
//! fixed per-node value derived from the node id, so neighbouring lights are not in
//! lock-step. Nothing about the signal is saved.

use cp_content::{LaneKind, RoadDefId};

use crate::connections::{Links, PedLinkKind, Turn};
use crate::graph::NetGraph;
use crate::junction::{Arm, Junction};
use crate::model::{JunctionControl, NodeId, SegmentId};

/// Green time per phase: 20 s at 20 ticks/s.
pub const GREEN_TICKS: u32 = 400;
/// Yellow (clearance) time per phase: 3 s.
pub const YELLOW_TICKS: u32 = 60;
/// Vehicle lanes a segment needs to count towards the auto-install rule.
pub const BIG_ROAD_LANES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Main,
    Yield,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Green,
    Yellow,
    Red,
}

/// One signal phase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase {
    /// Indices into `Links::connections` that go in this phase.
    pub connections: Vec<u16>,
    /// Indices into `Links::pedestrian` (crossings) that may walk in this phase.
    pub walk: Vec<u16>,
    /// A final phase added only so every crossing gets a walk time.
    pub pedestrian_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightPlan {
    pub phases: Vec<Phase>,
    /// Shift of this light's cycle, ticks.
    pub offset: u32,
}

impl LightPlan {
    pub fn phase_ticks() -> u32 {
        GREEN_TICKS + YELLOW_TICKS
    }

    pub fn cycle_ticks(&self) -> u32 {
        self.phases.len() as u32 * Self::phase_ticks()
    }

    /// Active phase index and whether it is still green, at game tick `tick`.
    pub fn phase_at(&self, tick: u64) -> (usize, bool) {
        let cycle = u64::from(self.cycle_ticks());
        let pos = (tick + u64::from(self.offset)) % cycle;
        let phase = (pos / u64::from(Self::phase_ticks())) as usize;
        let within = (pos % u64::from(Self::phase_ticks())) as u32;
        (phase, within < GREEN_TICKS)
    }

    /// Signal shown to connection `conn` at `tick`.
    pub fn connection_signal(&self, conn: u16, tick: u64) -> Signal {
        let (phase, green) = self.phase_at(tick);
        if self.phases[phase].connections.contains(&conn) {
            if green { Signal::Green } else { Signal::Yellow }
        } else {
            Signal::Red
        }
    }

    /// Whether pedestrians may start crossing `crossing` (index into `Links::pedestrian`)
    /// at `tick`: during the green part of a phase that lists it.
    pub fn walk(&self, crossing: u16, tick: u64) -> bool {
        let (phase, green) = self.phase_at(tick);
        green && self.phases[phase].walk.contains(&crossing)
    }
}

/// Derived control data of one junction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlPlan {
    /// Arm indices (into `Junction::arms`) of the main road: two, fewer for tiny nodes.
    pub main_arms: Vec<usize>,
    /// Per connection, aligned with `Links::connections`.
    pub priority: Vec<Priority>,
    /// Present when the node has a light (and something to signal).
    pub light: Option<LightPlan>,
}

fn vehicle_lanes(graph: &NetGraph, seg: SegmentId) -> usize {
    graph.segment(seg).map_or(0, |s| {
        s.lanes
            .iter()
            .filter_map(|&l| graph.lane(l))
            .filter(|l| matches!(l.kind, LaneKind::Car | LaneKind::Bus))
            .count()
    })
}

/// The §6.5 auto-install rule for a junction.
pub fn auto_light_rule(graph: &NetGraph, arms: &[Arm]) -> bool {
    arms.len() >= 3
        && arms
            .iter()
            .filter(|a| vehicle_lanes(graph, a.segment) >= BIG_ROAD_LANES)
            .count()
            >= 2
}

/// What the rule says for a node whose control is automatic, or `None` if the player's
/// choice must be kept or nothing changes.
pub fn auto_control(graph: &NetGraph, junction: &Junction) -> Option<JunctionControl> {
    let current = graph.node(junction.node)?.control;
    let wanted = if auto_light_rule(graph, &junction.arms) {
        JunctionControl::TrafficLight
    } else {
        JunctionControl::Priority
    };
    (!current.is_manual() && current != wanted).then_some(wanted)
}

/// Builds the control plan. `rank_of` gives the `RoadDef.rank` of a road type.
pub fn plan_control(
    graph: &NetGraph,
    junction: &Junction,
    rank_of: &dyn Fn(RoadDefId) -> u8,
) -> ControlPlan {
    let arms = &junction.arms;
    let links = &junction.links;
    let node = junction.node;
    let control = graph
        .node(node)
        .map_or(JunctionControl::Priority, |n| n.control);

    // Main road.
    let mut order: Vec<usize> = (0..arms.len()).collect();
    let rank = |a: &Arm| graph.segment(a.segment).map_or(0, |s| rank_of(s.road));
    order.sort_by(|&x, &y| {
        rank(&arms[y])
            .cmp(&rank(&arms[x]))
            .then(arms[y].width.total_cmp(&arms[x].width))
            .then(arms[x].segment.cmp(&arms[y].segment))
    });
    let main_arms: Vec<usize> = order.into_iter().take(2).collect();

    let arm_of = |lane| {
        let seg = graph.lane(lane).map(|l| l.segment);
        arms.iter().position(|a| Some(a.segment) == seg)
    };
    let from_arm: Vec<Option<usize>> = links.connections.iter().map(|c| arm_of(c.from)).collect();
    let to_arm: Vec<Option<usize>> = links.connections.iter().map(|c| arm_of(c.to)).collect();

    let priority = (0..links.connections.len())
        .map(|i| {
            if arms.len() <= 2 {
                return Priority::Main;
            }
            let on_main = from_arm[i].is_some_and(|a| main_arms.contains(&a));
            if !on_main {
                return Priority::Yield;
            }
            if links.connections[i].turn == Turn::Left {
                // Yield to oncoming straight traffic of the other main arm.
                let oncoming = (0..links.connections.len()).any(|j| {
                    links.connections[j].turn == Turn::Straight
                        && from_arm[j] != from_arm[i]
                        && from_arm[j].is_some_and(|a| main_arms.contains(&a))
                        && links.in_conflict(i as u16, j as u16)
                });
                if oncoming {
                    return Priority::Yield;
                }
            }
            Priority::Main
        })
        .collect();

    let light = control
        .has_light()
        .then(|| light_plan(graph, node, arms, links, &from_arm, &to_arm))
        .flatten();
    ControlPlan {
        main_arms,
        priority,
        light,
    }
}

fn light_plan(
    graph: &NetGraph,
    node: NodeId,
    arms: &[Arm],
    links: &Links,
    from_arm: &[Option<usize>],
    to_arm: &[Option<usize>],
) -> Option<LightPlan> {
    let n = links.connections.len();
    // Greedy colouring in index order.
    let mut colour: Vec<usize> = Vec::with_capacity(n);
    for i in 0..n {
        let used: Vec<usize> = colour
            .iter()
            .enumerate()
            .filter(|&(j, _)| links.in_conflict(i as u16, j as u16))
            .map(|(_, &c)| c)
            .collect();
        let c = (0..)
            .find(|c| !used.contains(c))
            .expect("a free colour exists");
        colour.push(c);
    }
    let count = colour.iter().max().map_or(0, |m| m + 1);
    let mut phases: Vec<Phase> = (0..count)
        .map(|c| Phase {
            connections: (0..n as u16).filter(|&i| colour[i as usize] == c).collect(),
            walk: Vec::new(),
            pedestrian_only: false,
        })
        .collect();

    // Crossings and the arm each one spans.
    let crossings: Vec<(u16, Option<usize>)> = links
        .pedestrian
        .iter()
        .enumerate()
        .filter(|(_, p)| p.kind == PedLinkKind::Crossing)
        .map(|(i, p)| {
            let seg = graph.lane(p.a).map(|l| l.segment);
            (i as u16, arms.iter().position(|a| Some(a.segment) == seg))
        })
        .collect();
    for phase in &mut phases {
        for &(ci, arm) in &crossings {
            let blocked = phase
                .connections
                .iter()
                .any(|&c| from_arm[c as usize] == arm || to_arm[c as usize] == arm);
            if !blocked {
                phase.walk.push(ci);
            }
        }
    }
    let uncovered = crossings
        .iter()
        .any(|(ci, _)| !phases.iter().any(|p| p.walk.contains(ci)));
    if uncovered {
        phases.push(Phase {
            connections: Vec::new(),
            walk: crossings.iter().map(|c| c.0).collect(),
            pedestrian_only: true,
        });
    }
    if phases.is_empty() {
        return None;
    }
    let cycle = phases.len() as u32 * LightPlan::phase_ticks();
    Some(LightPlan {
        phases,
        offset: node.index.wrapping_mul(97) % cycle,
    })
}

#[cfg(test)]
mod tests;

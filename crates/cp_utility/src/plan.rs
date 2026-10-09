//! Validation and planning of utility links — a **pure** function (`&` inputs only), like
//! `cp_net::validate_road_path`: the preview and `BuildLink` call the same code.
//!
//! **Snapping** per path vertex, in priority order: an existing node of the same network
//! within [`NODE_SNAP_RADIUS`]; otherwise a point on an existing link of the same network
//! within [`LINK_SNAP_RADIUS`] (the link is split there, so the networks connect); else a new
//! node. Pipes and power lines never snap to each other.
//!
//! **Legs**: straight lines between vertices. A leg shorter than the definition's
//! `min_link_m` is an error; one longer than `max_link_m` is cut into equal pieces.
//!
//! **Checks**: every sample (every [`SAMPLE_STEP`] m) must be on an owned tile, and a power
//! line must not pass through a building (footprint = the cells of its lot). Pipes may run
//! under buildings. A path whose later vertex lands within the snap radius of an earlier one
//! (a closed ring) reuses that node. All problems are returned together.
//!
//! **Cost**: `cp_net::segment_cost(length, cost_per_m)` per piece, summed (the §11 cents
//! helper).

use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingGrid, BuildingId};
use cp_content::UtilityDef;
use cp_core::Cost;
use cp_core::curve::{CubicBezier3, to_xz};
use cp_net::segment_cost;
use cp_terrain::{Terrain, TileOwnership};
use cp_zone::ZoneGrid;

use crate::graph::{UtilityGraph, UtilityLinkId, UtilityNet, UtilityNodeId};

pub const NODE_SNAP_RADIUS: f32 = 6.0;
pub const LINK_SNAP_RADIUS: f32 = 3.0;
pub const SAMPLE_STEP: f32 = 8.0;
/// Buildings farther than this from a leg's bounding circle are skipped quickly.
const BUILDING_REACH: f32 = 40.0;

#[derive(Clone, Debug, PartialEq)]
pub enum PathProblem {
    /// Fewer than two distinct points.
    PathTooShort,
    LinkTooShort {
        pos: Vec2,
        length: f32,
    },
    TileNotOwned {
        pos: Vec2,
    },
    /// A power line would run through this building.
    CrossesBuilding {
        building: BuildingId,
    },
    /// Both ends of a leg resolved to the same node.
    DegenerateLeg {
        pos: Vec2,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanNode {
    Existing(UtilityNodeId),
    /// Index into [`LinkPlan::new_nodes`].
    New(usize),
    /// Index into [`LinkPlan::splits`].
    Split(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlannedLink {
    pub a: PlanNode,
    pub b: PlanNode,
    pub length: f32,
}

/// An existing link to split at curve parameter `t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlannedSplit {
    pub link: UtilityLinkId,
    pub t: f32,
    pub pos: Vec3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkPlan {
    pub net: UtilityNet,
    pub links: Vec<PlannedLink>,
    pub new_nodes: Vec<Vec3>,
    pub splits: Vec<PlannedSplit>,
    pub cost: Cost,
}

/// What the planner reads besides the graph.
pub struct PlanInputs<'a> {
    pub tiles: &'a TileOwnership,
    pub terrain: Option<&'a Terrain>,
    pub buildings: Option<(&'a BuildingGrid, &'a ZoneGrid)>,
}

fn lift(w: &PlanInputs, p: Vec2) -> Vec3 {
    Vec3::new(p.x, w.terrain.map_or(0.0, |t| t.height(p)), p.y)
}

/// Whether the segment `a`–`b` passes through a building; returns the first one found.
fn building_on_leg(w: &PlanInputs, a: Vec2, b: Vec2) -> Option<BuildingId> {
    let (grid, zones) = w.buildings?;
    let mid = (a + b) * 0.5;
    let reach = a.distance(b) * 0.5 + BUILDING_REACH;
    for (id, bld) in grid.iter() {
        if bld.pos.distance(mid) > reach {
            continue;
        }
        let hit = bld.lot.cells().any(|cell| {
            zones
                .cell_footprint(cell)
                .is_some_and(|o| o.distance_to_segment(a, b) <= 0.0)
        });
        if hit {
            return Some(id);
        }
    }
    None
}

/// Plans a link path of `def`'s network, or returns every problem found.
pub fn plan_link_path(
    graph: &UtilityGraph,
    world: &PlanInputs,
    def: &UtilityDef,
    path: &[Vec2],
) -> Result<LinkPlan, Vec<PathProblem>> {
    let net = UtilityNet::of(def.kind);
    let mut pts: Vec<Vec2> = path.to_vec();
    pts.dedup_by(|a, b| a.distance(*b) < 0.01);
    if pts.len() < 2 {
        return Err(vec![PathProblem::PathTooShort]);
    }
    let mut new_nodes: Vec<Vec3> = Vec::new();
    let mut splits: Vec<PlannedSplit> = Vec::new();

    // Snap the vertices.
    let mut verts: Vec<(Vec2, PlanNode)> = Vec::new();
    for &p in &pts {
        if let Some(n) = graph.nearest_node(net, p, NODE_SNAP_RADIUS) {
            let pos = to_xz(graph.node(n).expect("exists").pos);
            verts.push((pos, PlanNode::Existing(n)));
        } else if let Some((l, t, _)) = graph.nearest_link(net, p, LINK_SNAP_RADIUS) {
            let pos3 = graph.link(l).expect("exists").curve.point(t);
            let pos = to_xz(pos3);
            let idx = splits
                .iter()
                .position(|s| s.link == l && s.pos.distance(pos3) < 0.5)
                .unwrap_or_else(|| {
                    splits.push(PlannedSplit {
                        link: l,
                        t,
                        pos: pos3,
                    });
                    splits.len() - 1
                });
            verts.push((pos, PlanNode::Split(idx)));
        } else if let Some(&(pos, node)) = verts
            .iter()
            .rev()
            .skip(1) // the previous vertex is a different point of the path
            .find(|(q, _)| q.distance(p) <= NODE_SNAP_RADIUS)
        {
            // A path that closes on itself reuses its own earlier node.
            verts.push((pos, node));
        } else {
            new_nodes.push(lift(world, p));
            verts.push((p, PlanNode::New(new_nodes.len() - 1)));
        }
    }

    let node_pos = |n: PlanNode, new_nodes: &[Vec3], splits: &[PlannedSplit]| -> Vec3 {
        match n {
            PlanNode::Existing(id) => graph.node(id).expect("exists").pos,
            PlanNode::New(i) => new_nodes[i],
            PlanNode::Split(i) => splits[i].pos,
        }
    };

    let mut problems = Vec::new();
    let mut links: Vec<PlannedLink> = Vec::new();
    for w in verts.windows(2) {
        let ((pa, na), (pb, nb)) = (w[0], w[1]);
        if na == nb {
            problems.push(PathProblem::DegenerateLeg { pos: pa });
            continue;
        }
        let len = pa.distance(pb);
        if len < def.min_link_m {
            problems.push(PathProblem::LinkTooShort {
                pos: pa,
                length: len,
            });
            continue;
        }
        // Ownership along the leg.
        let steps = (len / SAMPLE_STEP).ceil().max(1.0) as usize;
        if let Some(bad) = (0..=steps)
            .map(|k| pa.lerp(pb, k as f32 / steps as f32))
            .find(|q| !world.tiles.contains_point(*q))
        {
            problems.push(PathProblem::TileNotOwned { pos: bad });
        }
        if net == UtilityNet::Power
            && let Some(building) = building_on_leg(world, pa, pb)
        {
            problems.push(PathProblem::CrossesBuilding { building });
        }
        // Cut into equal pieces.
        let pieces = (len / def.max_link_m).ceil().max(1.0) as usize;
        let mut chain = vec![na];
        for k in 1..pieces {
            let q = pa.lerp(pb, k as f32 / pieces as f32);
            new_nodes.push(lift(world, q));
            chain.push(PlanNode::New(new_nodes.len() - 1));
        }
        chain.push(nb);
        for pair in chain.windows(2) {
            let (x, y) = (
                node_pos(pair[0], &new_nodes, &splits),
                node_pos(pair[1], &new_nodes, &splits),
            );
            links.push(PlannedLink {
                a: pair[0],
                b: pair[1],
                length: x.distance(y),
            });
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let cost = Cost(
        links
            .iter()
            .map(|l| segment_cost(l.length, def.cost_per_m).0)
            .sum(),
    );
    Ok(LinkPlan {
        net,
        links,
        new_nodes,
        splits,
        cost,
    })
}

/// Applies a plan: splits first (per link, ascending `t`), then nodes and links. Straight
/// links between plan nodes.
pub fn apply_plan(
    graph: &mut UtilityGraph,
    plan: &LinkPlan,
) -> Result<(), crate::graph::GraphError> {
    // Group the splits by link, remembering each split's plan index.
    let mut by_link: Vec<(UtilityLinkId, Vec<(f32, usize)>)> = Vec::new();
    for (i, s) in plan.splits.iter().enumerate() {
        match by_link.iter_mut().find(|(l, _)| *l == s.link) {
            Some((_, v)) => v.push((s.t, i)),
            None => by_link.push((s.link, vec![(s.t, i)])),
        }
    }
    let mut split_nodes: Vec<Option<UtilityNodeId>> = vec![None; plan.splits.len()];
    for (link, mut v) in by_link {
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-4);
        let ts: Vec<f32> = v.iter().map(|x| x.0).collect();
        let nodes = graph.split_link(link, &ts)?;
        for ((_, idx), n) in v.iter().zip(nodes) {
            split_nodes[*idx] = Some(n);
        }
    }
    let new_ids: Vec<UtilityNodeId> = plan
        .new_nodes
        .iter()
        .map(|&p| graph.add_node(plan.net, p))
        .collect();
    let resolve = |n: PlanNode| match n {
        PlanNode::Existing(id) => id,
        PlanNode::New(i) => new_ids[i],
        PlanNode::Split(i) => split_nodes[i].expect("split node created"),
    };
    for l in &plan.links {
        let (a, b) = (resolve(l.a), resolve(l.b));
        let (pa, pb) = (
            graph.node(a).expect("exists").pos,
            graph.node(b).expect("exists").pos,
        );
        graph.add_link(a, b, CubicBezier3::line(pa, pb))?;
    }
    Ok(())
}

//! Player commands of this domain (`architecture/system-design.md` §4, level 2).
//! New variants are only appended (save stability rule); adding a variant needs no
//! save version bump.
//!
//! [`apply`] validates with the same pure function the preview uses
//! ([`validate_road_path`]) and changes the world only after validation passed: an `Err`
//! leaves every resource untouched (the graph is also restored if applying the plan
//! fails half-way, which validation should make impossible).
//!
//! Reads from the world: [`NetGraph`], [`TileOwnership`], [`SimTerrain`] and
//! [`SimContent`] (all must be present; a sim without a map or content rejects build
//! commands with [`NetError::Missing`]). Money is not deducted here (budget task);
//! the cost is only returned.

use std::fmt;

use bevy_ecs::world::World;
use cp_content::{RoadDefId, SimContent};
use cp_core::Cost;
use cp_terrain::{SimTerrain, TileOwnership};
use serde::{Deserialize, Serialize};

use crate::build::{
    ElevationMode, RoadBuildError, RoadBuildPlan, RoadPath, RoadPathRequest, SnapOptions,
    segment_cost, validate_road_path,
};
use crate::graph::{NetGraph, NetOpError};
use crate::model::{Elevation, SegmentId};

/// Commands handled by this crate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum NetCommand {
    /// Builds a road along `path`. Snapping uses [`SnapOptions::default`] (node, segment
    /// and angle snaps on), so the command alone determines the result.
    BuildRoad {
        road: RoadDefId,
        path: RoadPath,
        elevation: ElevationMode,
    },
    /// Changes a segment's road type: same nodes and curve, lanes rebuilt.
    UpgradeSegment { seg: SegmentId, road: RoadDefId },
    /// Removes a segment (nodes left without segments disappear with it).
    RemoveSegment { seg: SegmentId },
}

/// Validation failure of a NetCommand; wrapped into `CommandError::Rejected` by the dispatcher.
#[derive(Clone, Debug, PartialEq)]
pub enum NetError {
    /// A resource the command needs is not in the world (`"terrain"`, `"content"`, ...).
    Missing(&'static str),
    /// `BuildRoad` failed validation; every problem found.
    Build(Vec<RoadBuildError>),
    UnknownSegment(SegmentId),
    UnknownRoad(RoadDefId),
    /// `UpgradeSegment` to the road the segment already has.
    SameRoad,
    /// The segment is elevated but the new road type has no elevated variant.
    NoElevatedVariant(RoadDefId),
    /// A graph operation failed while applying a validated plan (state was restored).
    Op(NetOpError),
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(what) => write!(f, "no {what} in the sim world"),
            Self::Build(errs) => {
                write!(f, "road not buildable: ")?;
                for (i, e) in errs.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{e:?}")?;
                }
                Ok(())
            }
            Self::UnknownSegment(s) => write!(f, "unknown segment {s:?}"),
            Self::UnknownRoad(r) => write!(f, "unknown road type {r:?}"),
            Self::SameRoad => write!(f, "segment already has this road type"),
            Self::NoElevatedVariant(r) => write!(f, "road type {r:?} has no elevated variant"),
            Self::Op(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for NetError {}

impl From<NetOpError> for NetError {
    fn from(e: NetOpError) -> Self {
        Self::Op(e)
    }
}

/// Validates and applies `cmd`, returning its cost.
pub fn apply(world: &mut World, cmd: &NetCommand) -> Result<Cost, NetError> {
    let content = || {
        world
            .get_resource::<SimContent>()
            .map(|c| c.0.clone())
            .ok_or(NetError::Missing("content"))
    };
    if !world.contains_resource::<NetGraph>() {
        return Err(NetError::Missing("net graph"));
    }
    match cmd {
        NetCommand::BuildRoad {
            road,
            path,
            elevation,
        } => {
            let content = content()?;
            let terrain = world
                .get_resource::<SimTerrain>()
                .map(|t| t.0.clone())
                .ok_or(NetError::Missing("terrain"))?;
            let tiles = world
                .get_resource::<TileOwnership>()
                .copied()
                .ok_or(NetError::Missing("tile ownership"))?;
            world.resource_scope(|_, mut graph: bevy_ecs::change_detection::Mut<NetGraph>| {
                let req = RoadPathRequest {
                    road: *road,
                    path: path.clone(),
                    elevation: *elevation,
                    snap: SnapOptions::default(),
                };
                let plan = validate_road_path(&graph, &terrain, &tiles, &content, &req)
                    .map_err(NetError::Build)?;
                let backup = graph.clone();
                match apply_plan(&mut graph, &content, &plan) {
                    Ok(()) => Ok(plan.cost),
                    Err(e) => {
                        *graph = backup;
                        Err(e.into())
                    }
                }
            })
        }
        NetCommand::UpgradeSegment { seg, road } => {
            let content = content()?;
            if road.index() >= content.roads().count() {
                return Err(NetError::UnknownRoad(*road));
            }
            let def = content.road(*road);
            let mut graph = world.resource_mut::<NetGraph>();
            let s = graph.segment(*seg).ok_or(NetError::UnknownSegment(*seg))?;
            if s.road == *road {
                return Err(NetError::SameRoad);
            }
            let cost_per_m = if s.elevation == Elevation::Elevated {
                def.elevated
                    .as_ref()
                    .ok_or(NetError::NoElevatedVariant(*road))?
                    .cost_per_m
            } else {
                def.cost_per_m
            };
            let cost = segment_cost(s.length, cost_per_m);
            graph.upgrade_segment(*seg, def, *road)?;
            Ok(cost)
        }
        NetCommand::RemoveSegment { seg } => {
            let mut graph = world.resource_mut::<NetGraph>();
            if graph.segment(*seg).is_none() {
                return Err(NetError::UnknownSegment(*seg));
            }
            graph.remove_segment(*seg)?;
            Ok(Cost::ZERO)
        }
    }
}

/// Applies a validated plan: splits first (per segment, ascending `t`), then the new
/// segments. Nodes are matched by position (`NetGraph::add_segment` reuses nodes within
/// `NODE_REUSE_EPS`, which covers existing, split and new nodes alike).
fn apply_plan(
    graph: &mut NetGraph,
    content: &cp_content::ContentDb,
    plan: &RoadBuildPlan,
) -> Result<(), NetOpError> {
    // Group the splits by original segment, in plan order of first appearance.
    let mut by_seg: Vec<(SegmentId, Vec<f32>)> = Vec::new();
    for sp in &plan.splits {
        match by_seg.iter_mut().find(|(s, _)| *s == sp.segment) {
            Some((_, ts)) => ts.push(sp.t),
            None => by_seg.push((sp.segment, vec![sp.t])),
        }
    }
    for (seg, mut ts) in by_seg {
        ts.sort_by(f32::total_cmp);
        ts.dedup();
        let road = graph
            .segment(seg)
            .ok_or(NetOpError::UnknownSegment(seg))?
            .road;
        graph.split_segments(content.road(road), seg, &ts)?;
    }
    let def = content.road(plan.road);
    for s in &plan.segments {
        graph.add_segment(def, plan.road, s.curve, s.elevation)?;
    }
    Ok(())
}

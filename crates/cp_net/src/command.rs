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
//! commands with [`NetError::Missing`]). Building and upgrading check the treasury after
//! validation and charge it once the change is applied ([`NetError::Funds`]: nothing changes,
//! nothing is charged).

use std::fmt;

use bevy_ecs::world::World;
use cp_content::{RoadDefId, SimContent};
use cp_core::treasury::{charge_funds, check_funds};
use cp_core::{Cost, InsufficientFunds};
use cp_terrain::{SimTerrain, TileOwnership};
use serde::{Deserialize, Serialize};

use crate::build::{
    ElevationMode, RoadBuildError, RoadBuildPlan, RoadPath, RoadPathRequest, SnapOptions,
    segment_cost, validate_road_path,
};
use crate::graph::{NetGraph, NetOpError};
use crate::model::{Elevation, NodeId, SegmentId};

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
    /// Installs or removes a traffic light at a junction (>= 3 segments). The choice is
    /// then manual: the auto-install rule leaves the node alone.
    ToggleTrafficLight { node: NodeId },
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
    /// The treasury cannot pay for the change.
    Funds(InsufficientFunds),
}

impl NetError {
    /// The shortfall, if that is why the command failed.
    pub fn insufficient_funds(&self) -> Option<InsufficientFunds> {
        match self {
            Self::Funds(f) => Some(*f),
            _ => None,
        }
    }
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
            Self::Funds(e) => write!(
                f,
                "insufficient funds: need {}, can spend {}",
                e.needed.0, e.available.0
            ),
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
            let cost = world.resource_scope(
                |world, mut graph: bevy_ecs::change_detection::Mut<NetGraph>| {
                    let req = RoadPathRequest {
                        road: *road,
                        path: path.clone(),
                        elevation: *elevation,
                        snap: SnapOptions::default(),
                    };
                    let plan = validate_road_path(&graph, &terrain, &tiles, &content, &req)
                        .map_err(NetError::Build)?;
                    check_funds(world, plan.cost).map_err(NetError::Funds)?;
                    // Everything is decided before the first change; a copy of the graph exists
                    // only in debug builds, to prove the check and the application agree.
                    check_plan(&graph, &plan).map_err(NetError::from)?;
                    #[cfg(debug_assertions)]
                    let backup = graph.clone();
                    match apply_plan(&mut graph, &content, &plan) {
                        Ok(()) => Ok(plan.cost),
                        Err(e) => {
                            #[cfg(debug_assertions)]
                            {
                                *graph = backup;
                                debug_assert!(false, "a checked plan failed to apply: {e}");
                            }
                            Err(NetError::from(e))
                        }
                    }
                },
            )?;
            charge_funds(world, cost);
            Ok(cost)
        }
        NetCommand::UpgradeSegment { seg, road } => {
            let content = content()?;
            if road.index() >= content.roads().count() {
                return Err(NetError::UnknownRoad(*road));
            }
            let def = content.road(*road);
            let graph = world.resource::<NetGraph>();
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
            check_funds(world, cost).map_err(NetError::Funds)?;
            let mut graph = world.resource_mut::<NetGraph>();
            graph.upgrade_segment(*seg, def, *road)?;
            charge_funds(world, cost);
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
        NetCommand::ToggleTrafficLight { node } => {
            world
                .resource_mut::<NetGraph>()
                .toggle_traffic_light(*node)?;
            Ok(Cost::ZERO)
        }
    }
}

/// The plan's splits grouped by original segment (plan order of first appearance), each
/// group's parameters sorted and deduplicated.
fn grouped_splits(plan: &RoadBuildPlan) -> Vec<(SegmentId, Vec<f32>)> {
    let mut by_seg: Vec<(SegmentId, Vec<f32>)> = Vec::new();
    for sp in &plan.splits {
        match by_seg.iter_mut().find(|(s, _)| *s == sp.segment) {
            Some((_, ts)) => ts.push(sp.t),
            None => by_seg.push((sp.segment, vec![sp.t])),
        }
    }
    for (_, ts) in &mut by_seg {
        ts.sort_by(f32::total_cmp);
        ts.dedup();
    }
    by_seg
}

/// Dry run of [`apply_plan`]: `Ok` exactly when applying the plan would succeed. Nothing is
/// changed, so no backup of the graph is needed to undo a half-applied plan.
pub(crate) fn check_plan(graph: &NetGraph, plan: &RoadBuildPlan) -> Result<(), NetOpError> {
    let curves: Vec<_> = plan.segments.iter().map(|s| s.curve).collect();
    graph.check_build(&grouped_splits(plan), &curves)
}

/// Applies a validated plan: splits first (per segment, ascending `t`), then the new
/// segments. Nodes are matched by position (`NetGraph::add_segment` reuses nodes within
/// `NODE_REUSE_EPS`, which covers existing, split and new nodes alike).
fn apply_plan(
    graph: &mut NetGraph,
    content: &cp_content::ContentDb,
    plan: &RoadBuildPlan,
) -> Result<(), NetOpError> {
    let by_seg = grouped_splits(plan);
    for (seg, ts) in by_seg {
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

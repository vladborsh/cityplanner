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
use bevy_math::{Vec2, Vec3};
use cp_content::{RoadClass, RoadDefId, SimContent};
use cp_core::curve::CubicBezier3;
use cp_core::treasury::{charge_funds, check_funds};
use cp_core::{Cost, InsufficientFunds};
use cp_terrain::{SimTerrain, TileOwnership};
use serde::{Deserialize, Serialize};

use crate::build::{
    ElevationMode, RoadBuildError, RoadBuildPlan, RoadPath, RoadPathRequest, SnapOptions,
    segment_cost, validate_build_plan, validate_road_path,
};
use crate::graph::{NetGraph, NetOpError};
use crate::model::{Elevation, NodeId, SegmentId};
use crate::plan::{Anchor, BuildPlan};

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
    /// Removes the part of a segment between the arc lengths `s0 < s1` measured from its
    /// `start` (§6.7). An end within [`PART_SNAP`] of the segment's start or end moves onto it
    /// (no remnant shorter than one zoning cell); both ends on the ends is `RemoveSegment`.
    /// The remnants keep the segment's orientation and road; the cut nodes become dead ends.
    /// Appended last: older logs read and replay as before.
    RemovePart { seg: SegmentId, s0: f32, s1: f32 },
    /// Turns a one-way road round (T-091): the segment keeps its place and its type, but now
    /// runs the other way. Fails with `NotOneWay` on a two-way road. Free.
    FlipSegment { seg: SegmentId },
    /// Builds a road from a resolved plan (§6.8.3, T-262): the tool has made every snap, the
    /// command applies what it is given. Appended last: older logs replay as before.
    BuildPlan { plan: BuildPlan },
    /// Changes the road type of the part of a segment between the arc lengths `s0 < s1` from its
    /// `start` (BUG-103: the upgrade tool takes the piece between the nearest corner, junction or
    /// section mark). The ends snap as `RemovePart`'s do; the segment is cut there and only the
    /// middle changes, charged by its length. Both ends on the ends is `UpgradeSegment`.
    /// Appended last: older logs read and replay as before.
    UpgradePart {
        seg: SegmentId,
        s0: f32,
        s1: f32,
        road: RoadDefId,
    },
}

/// A `RemovePart` end closer than this (metres) to an end of the segment moves onto that end.
pub const PART_SNAP: f32 = 8.0;

/// The stretch `RemovePart { s0, s1 }` takes from a segment of length `len`: each end within
/// [`PART_SNAP`] of an end of the segment moved onto it; `None` if nothing is left between them.
pub fn part_range(len: f32, s0: f32, s1: f32) -> Option<(f32, f32)> {
    let s0 = if s0 < PART_SNAP { 0.0 } else { s0 };
    let s1 = if s1 > len - PART_SNAP { len } else { s1 };
    (s0 < s1).then_some((s0, s1))
}

/// Checks that `seg` may become `road` (owned, another type of the same kind, a variant for
/// its elevation) and returns what a metre of it costs there.
fn upgrade_price(
    world: &World,
    content: &cp_content::ContentDb,
    seg: SegmentId,
    road: RoadDefId,
) -> Result<f32, NetError> {
    if road.index() >= content.roads().count() {
        return Err(NetError::UnknownRoad(road));
    }
    let def = content.road(road);
    check_owned(world, seg)?;
    let graph = world.resource::<NetGraph>();
    let s = graph.segment(seg).ok_or(NetError::UnknownSegment(seg))?;
    if s.road == road {
        return Err(NetError::SameRoad);
    }
    let was_rail = content.road(s.road).class == RoadClass::Rail;
    if was_rail != (def.class == RoadClass::Rail) {
        return Err(NetError::WrongKind(road));
    }
    match s.elevation {
        Elevation::Elevated => Ok(def
            .elevated
            .as_ref()
            .ok_or(NetError::NoElevatedVariant(road))?
            .cost_per_m),
        Elevation::Tunnel => Ok(def
            .tunnel
            .as_ref()
            .ok_or(NetError::NoTunnelVariant(road))?
            .cost_per_m),
        _ if !def.ground => Err(NetError::WrongKind(road)),
        _ => Ok(def.cost_per_m),
    }
}

/// What a metre of `def` costs to build at `elevation`: its elevated, bridge or tunnel variant's
/// price where it has one (as a plan prices it), else the ground price.
pub fn price_per_m(def: &cp_content::RoadDef, elevation: Elevation) -> f32 {
    match elevation {
        Elevation::Elevated => def
            .elevated
            .as_ref()
            .map_or(def.cost_per_m, |v| v.cost_per_m),
        Elevation::Bridge => def.bridge.as_ref().map_or(def.cost_per_m, |v| v.cost_per_m),
        Elevation::Tunnel => def.tunnel.as_ref().map_or(def.cost_per_m, |v| v.cost_per_m),
        Elevation::Ground => def.cost_per_m,
    }
}

/// What the road a removal takes would cost to build now (BUG-097): the whole segment
/// (`part: None`) or the stretch `RemovePart { s0, s1 }` takes, at the price of the segment's
/// road and elevation, so an upgrade counts at the road it upgraded to. `None` for an unknown
/// segment or an empty stretch.
pub fn removal_value(
    graph: &NetGraph,
    content: &cp_content::ContentDb,
    seg: SegmentId,
    part: Option<(f32, f32)>,
) -> Option<Cost> {
    let s = graph.segment(seg)?;
    let length = match part {
        None => s.length,
        Some((s0, s1)) => {
            let (s0, s1) = part_range(s.length, s0, s1)?;
            s1 - s0
        }
    };
    Some(segment_cost(
        length,
        price_per_m(content.road(s.road), s.elevation),
    ))
}

/// Validation failure of a NetCommand; wrapped into `CommandError::Rejected` by the dispatcher.
#[derive(Clone, Debug, PartialEq)]
pub enum NetError {
    /// A resource the command needs is not in the world (`"terrain"`, `"content"`, ...).
    Missing(&'static str),
    /// `BuildRoad` failed validation; every problem found.
    Build(Vec<RoadBuildError>),
    UnknownSegment(SegmentId),
    /// A `BuildPlan` anchor names a node the graph does not have.
    UnknownNode(NodeId),
    /// A `BuildPlan` without pieces, or with a point that is not finite.
    BadPlan,
    /// `RemovePart` with `s0 >= s1` or a non-finite length.
    BadPartRange,
    UnknownRoad(RoadDefId),
    /// `UpgradeSegment` to the road the segment already has.
    SameRoad,
    /// `FlipSegment` on a road that has lanes both ways.
    NotOneWay(SegmentId),
    /// The segment is elevated but the new road type has no elevated variant.
    NoElevatedVariant(RoadDefId),
    /// The segment is underground but the new road type has no tunnel variant.
    NoTunnelVariant(RoadDefId),
    /// `UpgradeSegment` between a road and a track type, or to a type that has no version on
    /// the ground for a ground segment.
    WrongKind(RoadDefId),
    /// A graph operation failed while applying a validated plan (state was restored).
    Op(NetOpError),
    /// The treasury cannot pay for the change.
    Funds(InsufficientFunds),
    /// `RemoveSegment` / `UpgradeSegment` on a segment whose curve leaves the bought tiles
    /// (design §5.1); `pos` is the first sampled point outside.
    NotOwned {
        seg: SegmentId,
        pos: Vec3,
    },
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
            Self::UnknownNode(n) => write!(f, "unknown node {n:?}"),
            Self::BadPlan => write!(f, "the build plan is empty or has a bad point"),
            Self::BadPartRange => write!(f, "part to remove is empty or not a range"),
            Self::UnknownRoad(r) => write!(f, "unknown road type {r:?}"),
            Self::SameRoad => write!(f, "segment already has this road type"),
            Self::NotOneWay(_) => write!(f, "only a one-way road can be turned round"),
            Self::NoElevatedVariant(r) => write!(f, "road type {r:?} has no elevated variant"),
            Self::NoTunnelVariant(r) => write!(f, "road type {r:?} has no tunnel variant"),
            Self::WrongKind(r) => write!(f, "road type {r:?} does not fit this segment"),
            Self::Op(e) => write!(f, "{e}"),
            Self::NotOwned { seg, .. } => write!(f, "segment {seg:?} is outside your land"),
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

/// Sample spacing of the ownership check along a segment's curve, metres.
const OWNERSHIP_STEP: f32 = 8.0;

/// The first point of `curve` (samples every [`OWNERSHIP_STEP`] metres, both ends included)
/// that lies on a tile that is not bought, if any. Used by `RemoveSegment` /
/// `UpgradeSegment` and by the tools' previews, so both agree.
pub fn curve_outside_owned(tiles: &TileOwnership, curve: &CubicBezier3) -> Option<Vec3> {
    let steps = ((curve.arc_table().length() / OWNERSHIP_STEP).ceil() as usize).max(1);
    (0..=steps).find_map(|k| {
        let p = curve.point(k as f32 / steps as f32);
        (!tiles.contains_point(Vec2::new(p.x, p.z))).then_some(p)
    })
}

/// Rejects a segment that leaves the bought tiles.
fn check_owned(world: &World, seg: SegmentId) -> Result<(), NetError> {
    let tiles = world
        .get_resource::<TileOwnership>()
        .ok_or(NetError::Missing("tile ownership"))?;
    let s = world
        .resource::<NetGraph>()
        .segment(seg)
        .ok_or(NetError::UnknownSegment(seg))?;
    match curve_outside_owned(tiles, &s.curve) {
        Some(pos) => Err(NetError::NotOwned { seg, pos }),
        None => Ok(()),
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
        } => build_paths(world, *road, *elevation, std::slice::from_ref(path)),
        NetCommand::BuildPlan { plan } => build_literal(world, plan),
        NetCommand::UpgradeSegment { seg, road } => {
            let content = content()?;
            let cost_per_m = upgrade_price(world, &content, *seg, *road)?;
            let def = content.road(*road);
            let len = world
                .resource::<NetGraph>()
                .segment(*seg)
                .map_or(0.0, |s| s.length);
            let cost = segment_cost(len, cost_per_m);
            check_funds(world, cost).map_err(NetError::Funds)?;
            let mut graph = world.resource_mut::<NetGraph>();
            graph.upgrade_segment(*seg, def, *road)?;
            charge_funds(world, cost);
            Ok(cost)
        }
        NetCommand::UpgradePart { seg, s0, s1, road } => {
            if !(s0.is_finite() && s1.is_finite() && s0 < s1) {
                return Err(NetError::BadPartRange);
            }
            let content = content()?;
            let cost_per_m = upgrade_price(world, &content, *seg, *road)?;
            let def = content.road(*road);
            let s = world
                .resource::<NetGraph>()
                .segment(*seg)
                .ok_or(NetError::UnknownSegment(*seg))?;
            let (len, old, curve) = (s.length, s.road, s.curve);
            let (s0, s1) = part_range(len, *s0, *s1).ok_or(NetError::BadPartRange)?;
            let cost = segment_cost(s1 - s0, cost_per_m);
            check_funds(world, cost).map_err(NetError::Funds)?;
            // Cut only where the part does not reach the end (as `RemovePart`); a split checks
            // all its pieces before it changes anything.
            let table = curve.arc_table();
            let mut ts = Vec::with_capacity(2);
            if s0 > 0.0 {
                ts.push(table.s_to_t(s0));
            }
            if s1 < len {
                ts.push(table.s_to_t(s1));
            }
            let mut graph = world.resource_mut::<NetGraph>();
            let middle = if ts.is_empty() {
                *seg
            } else {
                graph.split_segments(content.road(old), *seg, &ts)?.segments[usize::from(s0 > 0.0)]
            };
            graph.upgrade_segment(middle, def, *road)?;
            charge_funds(world, cost);
            Ok(cost)
        }
        NetCommand::FlipSegment { seg } => {
            let content = content()?;
            check_owned(world, *seg)?;
            let mut graph = world.resource_mut::<NetGraph>();
            let s = graph.segment(*seg).ok_or(NetError::UnknownSegment(*seg))?;
            let road = s.road;
            let def = content.road(road);
            if !def.is_one_way() {
                return Err(NetError::NotOneWay(*seg));
            }
            graph.flip_segment(*seg, def)?;
            Ok(Cost::ZERO)
        }
        NetCommand::RemoveSegment { seg } => {
            check_owned(world, *seg)?;
            let mut graph = world.resource_mut::<NetGraph>();
            graph.remove_segment(*seg)?;
            Ok(Cost::ZERO)
        }
        NetCommand::RemovePart { seg, s0, s1 } => {
            if !(s0.is_finite() && s1.is_finite() && s0 < s1) {
                return Err(NetError::BadPartRange);
            }
            let content = content()?;
            check_owned(world, *seg)?;
            let mut graph = world.resource_mut::<NetGraph>();
            let s = graph.segment(*seg).ok_or(NetError::UnknownSegment(*seg))?;
            let (len, road, curve) = (s.length, s.road, s.curve);
            let (s0, s1) = part_range(len, *s0, *s1).ok_or(NetError::BadPartRange)?;
            // Cuts only where the part does not already reach the end. A split checks all its
            // pieces before it changes anything, so a failure leaves the graph as it was.
            let table = curve.arc_table();
            let mut ts = Vec::with_capacity(2);
            if s0 > 0.0 {
                ts.push(table.s_to_t(s0));
            }
            if s1 < len {
                ts.push(table.s_to_t(s1));
            }
            if ts.is_empty() {
                graph.remove_segment(*seg)?;
            } else {
                let split = graph.split_segments(content.road(road), *seg, &ts)?;
                let middle = split.segments[usize::from(s0 > 0.0)];
                graph.remove_segment(middle)?;
            }
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

/// Applies a [`BuildPlan`] literally (T-262): the anchors are the truth, the sim snaps nothing.
/// Everything is decided (`validate_build_plan`, funds, the dry run) before the first change; the
/// graph keeps its invariants (§6.8.3 item 2) and no joint is merged away afterwards.
fn build_literal(world: &mut World, plan: &BuildPlan) -> Result<Cost, NetError> {
    let content = world
        .get_resource::<SimContent>()
        .map(|c| c.0.clone())
        .ok_or(NetError::Missing("content"))?;
    let terrain = world
        .get_resource::<SimTerrain>()
        .map(|t| t.0.clone())
        .ok_or(NetError::Missing("terrain"))?;
    let tiles = world
        .get_resource::<TileOwnership>()
        .copied()
        .ok_or(NetError::Missing("tile ownership"))?;
    if plan.pieces.is_empty() {
        return Err(NetError::BadPlan);
    }
    {
        let graph = world.resource::<NetGraph>();
        let known = |a: &Anchor| match a {
            Anchor::Free(p) if p.is_finite() => Ok(()),
            Anchor::Free(_) => Err(NetError::BadPlan),
            Anchor::Node(n) => graph.node(*n).map(|_| ()).ok_or(NetError::UnknownNode(*n)),
            Anchor::OnSegment(s, t) if t.is_finite() => graph
                .segment(*s)
                .map(|_| ())
                .ok_or(NetError::UnknownSegment(*s)),
            Anchor::OnSegment(..) => Err(NetError::BadPlan),
        };
        for piece in &plan.pieces {
            if !(piece.c1.is_finite() && piece.c2.is_finite()) {
                return Err(NetError::BadPlan);
            }
            known(&piece.from)?;
            known(&piece.to)?;
        }
    }
    let cost = world.resource_scope(
        |world, mut graph: bevy_ecs::change_detection::Mut<NetGraph>| {
            let built = validate_build_plan(&graph, &terrain, &tiles, &content, plan)
                .map_err(NetError::Build)?;
            check_funds(world, built.cost).map_err(NetError::Funds)?;
            check_plan(&graph, &content, &built).map_err(NetError::from)?;
            #[cfg(debug_assertions)]
            let (backup, before) = (graph.clone(), graph.check_basic_invariants().len());
            match apply_plan_with(&mut graph, &content, &built, false) {
                Ok(()) => {
                    #[cfg(debug_assertions)]
                    {
                        let after = graph.check_basic_invariants();
                        debug_assert!(
                            after.len() <= before,
                            "a build plan broke a graph invariant: {after:?}"
                        );
                        let _ = backup;
                    }
                    Ok(built.cost)
                }
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

/// The ground strips the road command `cmd` would lay (BUG-096): each new or upgraded ground
/// segment's curve with the half width of its road (asphalt and sidewalks). Pure, from the same
/// validators [`apply`] uses; empty for a command that lays no road or would be refused.
pub fn command_strips(
    graph: &NetGraph,
    terrain: &cp_terrain::Terrain,
    tiles: &TileOwnership,
    content: &cp_content::ContentDb,
    cmd: &NetCommand,
) -> Vec<(CubicBezier3, f32)> {
    let planned = |plan: RoadBuildPlan| -> Vec<(CubicBezier3, f32)> {
        let hw = content.road(plan.road).width * 0.5;
        plan.segments
            .iter()
            .filter(|s| s.elevation == Elevation::Ground)
            .map(|s| (s.curve, hw))
            .collect()
    };
    match cmd {
        NetCommand::BuildRoad {
            road,
            path,
            elevation,
        } => {
            let req = RoadPathRequest {
                road: *road,
                path: path.clone(),
                elevation: *elevation,
                snap: SnapOptions::default(),
            };
            validate_road_path(graph, terrain, tiles, content, &req).map_or(Vec::new(), planned)
        }
        NetCommand::BuildPlan { plan } => {
            validate_build_plan(graph, terrain, tiles, content, plan).map_or(Vec::new(), planned)
        }
        NetCommand::UpgradeSegment { seg, road } => match graph.segment(*seg) {
            Some(s)
                if s.elevation == Elevation::Ground && road.index() < content.roads().count() =>
            {
                vec![(s.curve, content.road(*road).width * 0.5)]
            }
            _ => Vec::new(),
        },
        NetCommand::UpgradePart { seg, s0, s1, road } => match graph.segment(*seg) {
            Some(s)
                if s.elevation == Elevation::Ground && road.index() < content.roads().count() =>
            {
                match part_range(s.length, *s0, *s1) {
                    Some((a, b)) => {
                        let table = s.curve.arc_table();
                        let curve = s.curve.sub_curve(table.s_to_t(a), table.s_to_t(b));
                        vec![(curve, content.road(*road).width * 0.5)]
                    }
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Builds `paths` one after another with the existing validator (`BuildRoad` semantics): all
/// or nothing, charged once.
fn build_paths(
    world: &mut World,
    road: RoadDefId,
    elevation: ElevationMode,
    paths: &[RoadPath],
) -> Result<Cost, NetError> {
    let content = world
        .get_resource::<SimContent>()
        .map(|c| c.0.clone())
        .ok_or(NetError::Missing("content"))?;
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
            let backup = graph.clone();
            let mut total = Cost::ZERO;
            for path in paths {
                let req = RoadPathRequest {
                    road,
                    path: path.clone(),
                    elevation,
                    snap: SnapOptions::default(),
                };
                let step = (|| {
                    let plan = validate_road_path(&graph, &terrain, &tiles, &content, &req)
                        .map_err(NetError::Build)?;
                    check_funds(world, Cost(total.0 + plan.cost.0)).map_err(NetError::Funds)?;
                    // Everything is decided before the first change of this piece.
                    check_plan(&graph, &content, &plan).map_err(NetError::from)?;
                    apply_plan(&mut graph, &content, &plan).map_err(NetError::from)?;
                    Ok::<_, NetError>(plan.cost)
                })();
                match step {
                    Ok(c) => total = Cost(total.0 + c.0),
                    Err(e) => {
                        *graph = backup;
                        return Err(e);
                    }
                }
            }
            Ok(total)
        },
    )?;
    charge_funds(world, cost);
    Ok(cost)
}

/// Builds a road along `path` for the game itself, not for the player: tile ownership is not
/// checked (the entry highway crosses tiles that are not bought) and nothing is charged.
/// Snaps are off except onto existing nodes. Same validation and application as
/// [`NetCommand::BuildRoad`] otherwise; used by `cp_sim::new_game` (`system-design.md` §5.1).
pub fn build_unowned(world: &mut World, road: RoadDefId, path: RoadPath) -> Result<(), NetError> {
    let content = world
        .get_resource::<SimContent>()
        .map(|c| c.0.clone())
        .ok_or(NetError::Missing("content"))?;
    let terrain = world
        .get_resource::<SimTerrain>()
        .map(|t| t.0.clone())
        .ok_or(NetError::Missing("terrain"))?;
    let mut tiles = TileOwnership::none();
    for tz in 0..cp_terrain::TILES_PER_SIDE {
        for tx in 0..cp_terrain::TILES_PER_SIDE {
            tiles.set(tx, tz, true);
        }
    }
    world.resource_scope(|_, mut graph: bevy_ecs::change_detection::Mut<NetGraph>| {
        let req = RoadPathRequest {
            road,
            path,
            elevation: ElevationMode::Ground,
            snap: SnapOptions {
                node: true,
                segment: false,
                angle: false,
                zone_grid: false,
            },
        };
        let plan = validate_road_path(&graph, &terrain, &tiles, &content, &req)
            .map_err(NetError::Build)?;
        check_plan(&graph, &content, &plan).map_err(NetError::from)?;
        apply_plan(&mut graph, &content, &plan).map_err(NetError::from)
    })
}

/// Validates a road for a caller that builds it together with something else (a station's
/// platform, T-162): the checks of [`NetCommand::BuildRoad`] including ownership, but nothing
/// is charged or changed. Hand the plan to [`apply_build_plan`] with the graph untouched in
/// between.
pub fn plan_build(world: &World, req: &RoadPathRequest) -> Result<RoadBuildPlan, NetError> {
    let content = world
        .get_resource::<SimContent>()
        .ok_or(NetError::Missing("content"))?;
    let terrain = world
        .get_resource::<SimTerrain>()
        .ok_or(NetError::Missing("terrain"))?;
    let tiles = world
        .get_resource::<TileOwnership>()
        .ok_or(NetError::Missing("tile ownership"))?;
    let graph = world
        .get_resource::<NetGraph>()
        .ok_or(NetError::Missing("net graph"))?;
    let plan =
        validate_road_path(graph, &terrain.0, tiles, &content.0, req).map_err(NetError::Build)?;
    check_plan(graph, &content.0, &plan).map_err(NetError::from)?;
    Ok(plan)
}

/// Applies a plan of [`plan_build`] (no charge) and gives the ids of its segments in plan
/// order.
pub fn apply_build_plan(
    world: &mut World,
    plan: &RoadBuildPlan,
) -> Result<Vec<SegmentId>, NetError> {
    let content = world
        .get_resource::<SimContent>()
        .map(|c| c.0.clone())
        .ok_or(NetError::Missing("content"))?;
    let mut graph = world
        .get_resource_mut::<NetGraph>()
        .ok_or(NetError::Missing("net graph"))?;
    let _ = dissolve(&mut graph, &content, plan);
    for (seg, ts) in grouped_splits(plan) {
        let road = graph
            .segment(seg)
            .ok_or(NetOpError::UnknownSegment(seg))?
            .road;
        graph.split_segments(content.road(road), seg, &ts)?;
    }
    let def = content.road(plan.road);
    let mut ids = Vec::with_capacity(plan.segments.len());
    for s in &plan.segments {
        ids.push(graph.add_segment(def, plan.road, s.curve, s.elevation)?);
    }
    merge_joints_near_junctions(&mut graph, &content, &ids);
    Ok(ids)
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

/// The net as it would be after `plan` (a clone with the plan applied, no charge); `None` when
/// the plan does not apply. What the road tool's zone preview lays its cells on (BUG-047).
pub fn plan_applied(
    graph: &NetGraph,
    content: &cp_content::ContentDb,
    plan: &RoadBuildPlan,
) -> Option<NetGraph> {
    let mut after = graph.clone();
    apply_plan_with(&mut after, content, plan, !plan.literal).ok()?;
    Some(after)
}

/// Dry run of [`apply_plan`]: `Ok` exactly when applying the plan would succeed. Nothing is
/// changed, so no backup of the graph is needed to undo a half-applied plan.
pub(crate) fn check_plan(
    graph: &NetGraph,
    content: &cp_content::ContentDb,
    plan: &RoadBuildPlan,
) -> Result<(), NetOpError> {
    let curves: Vec<_> = plan.segments.iter().map(|s| s.curve).collect();
    if plan.dissolve.is_empty() {
        return graph.check_build(&grouped_splits(plan), &curves);
    }
    // The cuts the plan dissolves are merged first; the rest is decided on that graph.
    let mut work = graph.clone();
    let _ = dissolve(&mut work, content, plan);
    work.check_build(&grouped_splits(plan), &curves)
}

/// Merges the cuts of `plan.dissolve` away, in order (the road of a cut is the road of its
/// pieces).
fn dissolve(
    graph: &mut NetGraph,
    content: &cp_content::ContentDb,
    plan: &RoadBuildPlan,
) -> Vec<SegmentId> {
    let mut made = Vec::new();
    for &node in &plan.dissolve {
        let Some(road) = graph
            .node(node)
            .and_then(|n| n.segments.first())
            .and_then(|&s| graph.segment(s))
            .map(|s| s.road)
        else {
            continue;
        };
        made.extend(graph.merge_joint_planar(content.road(road), node));
    }
    made
}

/// Applies a validated plan: splits first (per segment, ascending `t`), then the new
/// segments. Nodes are matched by position (`NetGraph::add_segment` reuses nodes within
/// `NODE_REUSE_EPS`, which covers existing, split and new nodes alike).
fn apply_plan(
    graph: &mut NetGraph,
    content: &cp_content::ContentDb,
    plan: &RoadBuildPlan,
) -> Result<(), NetOpError> {
    apply_plan_with(graph, content, plan, true)
}

/// [`apply_plan`]; `merge` is the BUG-069 pass over the joints of the junctions the plan touched
/// (the older `BuildRoad`; a literal `BuildPlan` has no such pass, its cuts are made right).
fn apply_plan_with(
    graph: &mut NetGraph,
    content: &cp_content::ContentDb,
    plan: &RoadBuildPlan,
    merge: bool,
) -> Result<(), NetOpError> {
    // The plan's splits name the streets the cuts were merged into in the validator's copy of the
    // graph: the arena hands out the same slots here, which this checks.
    let made = dissolve(graph, content, plan);
    debug_assert_eq!(
        made, plan.merged,
        "the merged streets differ from the validated ones"
    );
    let by_seg = grouped_splits(plan);
    for (seg, ts) in by_seg {
        let road = graph
            .segment(seg)
            .ok_or(NetOpError::UnknownSegment(seg))?
            .road;
        graph.split_segments(content.road(road), seg, &ts)?;
    }
    let def = content.road(plan.road);
    let mut ids = Vec::with_capacity(plan.segments.len());
    for s in &plan.segments {
        ids.push(graph.add_segment(def, plan.road, s.curve, s.elevation)?);
    }
    if merge {
        merge_joints_near_junctions(graph, content, &ids);
    }
    Ok(())
}

/// A piece of a road shorter than this, metres, between a junction and an automatic cut of
/// the road leaves too little room for the junction's corners and zebra crossing (BUG-069).
fn joint_reach(def: &cp_content::RoadDef) -> f32 {
    def.width * 1.5 + 6.0
}

/// BUG-069: a crossing built a few metres from an automatic cut of the road it crosses (the
/// cuts lie every 64 m, crossings on the 8 m zone lattice) leaves a piece too short for the
/// junction's setback: its corners and zebra crossing went missing. The cut goes: the short
/// piece and the next one become one straight segment (see [`NetGraph::merge_joint`]). Only
/// junctions the edit touched (the dirty ones) are looked at; segments in `keep` stay.
fn merge_joints_near_junctions(
    graph: &mut NetGraph,
    content: &cp_content::ContentDb,
    keep: &[SegmentId],
) {
    let mut joints: Vec<(NodeId, RoadDefId)> = Vec::new();
    for node in graph.dirty_nodes() {
        let Some(n) = graph.node(node) else { continue };
        if n.segments.len() < 3 {
            continue;
        }
        for &sid in &n.segments {
            let Some(seg) = graph.segment(sid) else {
                continue;
            };
            let joint = if seg.start == node {
                seg.end
            } else {
                seg.start
            };
            let others_kept = graph
                .node(joint)
                .is_some_and(|j| j.segments.iter().any(|s| keep.contains(s)));
            if keep.contains(&sid)
                || others_kept
                || seg.length >= joint_reach(content.road(seg.road))
                || joints.iter().any(|(j, _)| *j == joint)
            {
                continue;
            }
            joints.push((joint, seg.road));
        }
    }
    for (joint, road) in joints {
        graph.merge_joint(content.road(road), joint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_content::ContentDb;
    use cp_core::treasury::Treasury;

    fn world(open: &[(usize, usize)]) -> (World, RoadDefId) {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let road = db.road_id("basic_2l").unwrap();
        let mut w = World::new();
        w.insert_resource(SimContent(std::sync::Arc::new(db)));
        w.insert_resource(NetGraph::new());
        let mut tiles = TileOwnership::none();
        for &(tx, tz) in open {
            tiles.set(tx, tz, true);
        }
        w.insert_resource(tiles);
        w.insert_resource(Treasury::default());
        (w, road)
    }

    fn add(w: &mut World, road: RoadDefId, x0: f32, x1: f32) -> SegmentId {
        let def = w.resource::<SimContent>().0.road(road).clone();
        w.resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(x0, 0.0, 1000.0), Vec3::new(x1, 0.0, 1000.0)),
                Elevation::Ground,
            )
            .unwrap()
    }

    #[test]
    fn removing_or_upgrading_outside_the_bought_tiles_is_not_owned() {
        let t = cp_terrain::TILE_SIZE;
        let (tx, tz) = cp_terrain::Terrain::tile_of(Vec2::new(100.0, 1000.0));
        let (mut w, road) = world(&[(tx, tz)]);
        let other = w.resource::<SimContent>().0.road_id("medium_4l").unwrap();
        let half = add(&mut w, road, t - 50.0, t + 50.0);
        let inside = add(&mut w, road, 100.0, 200.0);

        let up = NetCommand::UpgradeSegment {
            seg: half,
            road: other,
        };
        assert!(matches!(
            apply(&mut w, &up),
            Err(NetError::NotOwned { seg, .. }) if seg == half
        ));
        assert!(matches!(
            apply(&mut w, &NetCommand::RemoveSegment { seg: half }),
            Err(NetError::NotOwned { .. })
        ));
        assert!(w.resource::<NetGraph>().segment(half).is_some());
        assert_eq!(w.resource::<NetGraph>().segment(half).unwrap().road, road);

        apply(&mut w, &NetCommand::RemoveSegment { seg: inside }).unwrap();
        assert!(w.resource::<NetGraph>().segment(inside).is_none());
    }

    /// BUG-069: a crossing 8 m from the joint of a straight road takes the joint away; a bend
    /// and a joint far enough away stay.
    #[test]
    fn a_crossing_next_to_a_joint_merges_the_joint_away() {
        let (mut w, road) = world(&[]);
        let db = w.resource::<SimContent>().0.clone();
        let def = db.road(road).clone();
        add(&mut w, road, 0.0, 64.0);
        add(&mut w, road, 64.0, 128.0);
        add(&mut w, road, 128.0, 200.0);
        let mut g = w.resource_mut::<NetGraph>();
        // A cross street at x = 72: 8 m past the joint at 64, 56 m before the one at 128.
        let (sid, _) = g.segments().find(|(_, s)| s.curve.p0.x == 64.0).unwrap();
        g.split_segment(&def, sid, 0.125).unwrap();
        for z in [900.0, 1100.0] {
            let line = CubicBezier3::line(Vec3::new(72.0, 0.0, z), Vec3::new(72.0, 0.0, 1000.0));
            g.add_segment(&def, road, line, Elevation::Ground).unwrap();
        }
        merge_joints_near_junctions(&mut g, &db, &[]);
        let joints: Vec<f32> = g
            .nodes()
            .filter(|(_, n)| n.kind == crate::model::NodeKind::Middle)
            .map(|(_, n)| n.pos.x)
            .collect();
        assert_eq!(
            joints,
            vec![128.0],
            "the joint 8 m away goes, the one 56 m away stays"
        );
        assert!(
            g.segments()
                .any(|(_, s)| s.curve.p0.x == 0.0 && (s.curve.p3.x - 72.0).abs() < 1e-3)
        );
    }

    #[test]
    fn a_bend_or_a_road_change_is_no_joint_to_merge() {
        let (mut w, road) = world(&[]);
        let db = w.resource::<SimContent>().0.clone();
        let def = db.road(road).clone();
        let other = db.road_id("medium_4l").unwrap();
        let mut g = w.resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(x, 0.0, z);
        g.add_segment(
            &def,
            road,
            CubicBezier3::line(p(0.0, 0.0), p(50.0, 0.0)),
            Elevation::Ground,
        )
        .unwrap();
        g.add_segment(
            &def,
            road,
            CubicBezier3::line(p(50.0, 0.0), p(90.0, 30.0)),
            Elevation::Ground,
        )
        .unwrap();
        g.add_segment(
            &def,
            road,
            CubicBezier3::line(p(0.0, 100.0), p(50.0, 100.0)),
            Elevation::Ground,
        )
        .unwrap();
        g.add_segment(
            db.road(other),
            other,
            CubicBezier3::line(p(50.0, 100.0), p(100.0, 100.0)),
            Elevation::Ground,
        )
        .unwrap();
        let bend = g.find_node(p(50.0, 0.0)).unwrap();
        let change = g.find_node(p(50.0, 100.0)).unwrap();
        assert!(g.merge_joint(&def, bend).is_none());
        assert!(g.merge_joint(&def, change).is_none());
        assert_eq!(g.segment_count(), 4);
    }

    fn lengths(w: &World) -> Vec<(f32, f32)> {
        let g = w.resource::<NetGraph>();
        let mut v: Vec<_> = g
            .segments()
            .map(|(_, s)| (s.curve.p0.x, s.length))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    }

    fn part_world() -> (World, SegmentId) {
        let (tx, tz) = cp_terrain::Terrain::tile_of(Vec2::new(100.0, 1000.0));
        let (mut w, road) = world(&[(tx, tz)]);
        let seg = add(&mut w, road, 100.0, 190.0);
        (w, seg)
    }

    #[test]
    fn upgrade_part_changes_only_the_middle_and_charges_its_length() {
        let (mut w, seg) = part_world();
        let db = w.resource::<SimContent>().0.clone();
        let (basic, medium) = (
            db.road_id("basic_2l").unwrap(),
            db.road_id("medium_4l").unwrap(),
        );
        let before = w.resource::<Treasury>().balance;
        let cost = apply(
            &mut w,
            &NetCommand::UpgradePart {
                seg,
                s0: 20.0,
                s1: 60.0,
                road: medium,
            },
        )
        .unwrap();
        assert_eq!(cost, segment_cost(40.0, db.road(medium).cost_per_m));
        assert_eq!(w.resource::<Treasury>().balance, Cost(before.0 - cost.0));
        let g = w.resource::<NetGraph>();
        let mut v: Vec<_> = g
            .segments()
            .map(|(_, s)| (s.curve.p0.x, s.length, s.road, s.lanes.len()))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(v.len(), 3, "cut at both ends of the part");
        assert_eq!([v[0].2, v[1].2, v[2].2], [basic, medium, basic]);
        assert!((v[1].1 - 40.0).abs() < 0.01 && (v[1].0 - 120.0).abs() < 0.01);
        assert!(
            v[1].3 > v[0].3,
            "the middle's lanes rebuilt for the new type"
        );
        // Joined, not cut: the remnants and the middle share their nodes.
        assert_eq!(g.node_count(), 4);
    }

    #[test]
    fn upgrade_part_near_both_ends_is_the_whole_segment() {
        let (mut w, seg) = part_world();
        let medium = w.resource::<SimContent>().0.road_id("medium_4l").unwrap();
        apply(
            &mut w,
            &NetCommand::UpgradePart {
                seg,
                s0: 3.0,
                s1: 86.0,
                road: medium,
            },
        )
        .unwrap();
        let g = w.resource::<NetGraph>();
        assert_eq!(g.segments().count(), 1);
        assert_eq!(g.segment(seg).unwrap().road, medium);
    }

    #[test]
    fn remove_part_cuts_out_the_middle_and_leaves_dead_ends() {
        let (mut w, seg) = part_world();
        let lanes = w.resource::<NetGraph>().segment(seg).unwrap().lanes.len();
        let cost = apply(
            &mut w,
            &NetCommand::RemovePart {
                seg,
                s0: 20.0,
                s1: 60.0,
            },
        )
        .unwrap();
        assert_eq!(cost, Cost::ZERO);
        let v = lengths(&w);
        assert_eq!(v.len(), 2);
        assert!((v[0].1 - 20.0).abs() < 0.01 && (v[0].0 - 100.0).abs() < 0.01);
        assert!((v[1].1 - 30.0).abs() < 0.01 && (v[1].0 - 160.0).abs() < 0.01);
        let g = w.resource::<NetGraph>();
        assert_eq!(g.node_count(), 4);
        for (_, s) in g.segments() {
            assert_eq!(s.lanes.len(), lanes, "lanes rebuilt");
            assert!(s.curve.p3.x > s.curve.p0.x, "still start to end");
            for n in [s.start, s.end] {
                assert!(g.node(n).unwrap().segments.len() == 1);
                assert_eq!(g.node(n).unwrap().kind, crate::model::NodeKind::End);
            }
        }
    }

    #[test]
    fn remove_part_near_an_end_takes_the_remnant_with_it() {
        let (mut w, seg) = part_world();
        apply(
            &mut w,
            &NetCommand::RemovePart {
                seg,
                s0: 3.0,
                s1: 60.0,
            },
        )
        .unwrap();
        let v = lengths(&w);
        assert_eq!(v.len(), 1);
        assert!((v[0].1 - 30.0).abs() < 0.01 && (v[0].0 - 160.0).abs() < 0.01);

        let (mut w, seg) = part_world();
        apply(
            &mut w,
            &NetCommand::RemovePart {
                seg,
                s0: 30.0,
                s1: 85.0,
            },
        )
        .unwrap();
        let v = lengths(&w);
        assert_eq!(v.len(), 1);
        assert!((v[0].1 - 30.0).abs() < 0.01 && (v[0].0 - 100.0).abs() < 0.01);
    }

    #[test]
    fn remove_part_of_the_whole_length_is_remove_segment() {
        let (mut w, seg) = part_world();
        let len = w.resource::<NetGraph>().segment(seg).unwrap().length;
        let (mut r, rseg) = part_world();
        apply(
            &mut w,
            &NetCommand::RemovePart {
                seg,
                s0: 0.0,
                s1: len,
            },
        )
        .unwrap();
        apply(&mut r, &NetCommand::RemoveSegment { seg: rseg }).unwrap();
        assert_eq!(w.resource::<NetGraph>(), r.resource::<NetGraph>());
        assert_eq!(w.resource::<NetGraph>().segment_count(), 0);
    }

    #[test]
    fn remove_part_with_a_bad_range_or_segment_changes_nothing() {
        let (mut w, seg) = part_world();
        let before = w.resource::<NetGraph>().clone();
        for (s0, s1) in [
            (60.0, 20.0),
            (40.0, 40.0),
            (f32::NAN, 50.0),
            (10.0, f32::INFINITY),
        ] {
            let r = apply(&mut w, &NetCommand::RemovePart { seg, s0, s1 });
            assert!(matches!(r, Err(NetError::BadPartRange)), "{s0} {s1}");
        }
        // Both ends snap to the ends only if the range is a range: 3 < 5 < 8 collapses.
        let r = apply(
            &mut w,
            &NetCommand::RemovePart {
                seg,
                s0: 90.0,
                s1: 95.0,
            },
        );
        assert!(r.is_err());
        let ghost = SegmentId::new(60_000, 0);
        let r = apply(
            &mut w,
            &NetCommand::RemovePart {
                seg: ghost,
                s0: 0.0,
                s1: 50.0,
            },
        );
        assert!(r.is_err());
        assert_eq!(w.resource::<NetGraph>(), &before);
    }

    #[test]
    fn remove_part_outside_the_bought_tiles_is_not_owned() {
        let t = cp_terrain::TILE_SIZE;
        let (tx, tz) = cp_terrain::Terrain::tile_of(Vec2::new(100.0, 1000.0));
        let (mut w, road) = world(&[(tx, tz)]);
        let half = add(&mut w, road, t - 50.0, t + 50.0);
        let r = apply(
            &mut w,
            &NetCommand::RemovePart {
                seg: half,
                s0: 20.0,
                s1: 60.0,
            },
        );
        assert!(matches!(r, Err(NetError::NotOwned { .. })));
        assert_eq!(w.resource::<NetGraph>().segment_count(), 1);
    }
}

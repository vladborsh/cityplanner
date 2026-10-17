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
    /// Removes the part of a segment between the arc lengths `s0 < s1` measured from its
    /// `start` (§6.7). An end within [`PART_SNAP`] of the segment's start or end moves onto it
    /// (no remnant shorter than one zoning cell); both ends on the ends is `RemoveSegment`.
    /// The remnants keep the segment's orientation and road; the cut nodes become dead ends.
    /// Appended last: older logs read and replay as before.
    RemovePart { seg: SegmentId, s0: f32, s1: f32 },
    /// Turns a one-way road round (T-091): the segment keeps its place and its type, but now
    /// runs the other way. Fails with `NotOneWay` on a two-way road. Free.
    FlipSegment { seg: SegmentId },
}

/// A `RemovePart` end closer than this (metres) to an end of the segment moves onto that end.
pub const PART_SNAP: f32 = 8.0;

/// Validation failure of a NetCommand; wrapped into `CommandError::Rejected` by the dispatcher.
#[derive(Clone, Debug, PartialEq)]
pub enum NetError {
    /// A resource the command needs is not in the world (`"terrain"`, `"content"`, ...).
    Missing(&'static str),
    /// `BuildRoad` failed validation; every problem found.
    Build(Vec<RoadBuildError>),
    UnknownSegment(SegmentId),
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
            check_owned(world, *seg)?;
            let graph = world.resource::<NetGraph>();
            let s = graph.segment(*seg).ok_or(NetError::UnknownSegment(*seg))?;
            if s.road == *road {
                return Err(NetError::SameRoad);
            }
            let was_rail = content.road(s.road).class == RoadClass::Rail;
            if was_rail != (def.class == RoadClass::Rail) {
                return Err(NetError::WrongKind(*road));
            }
            let cost_per_m = match s.elevation {
                Elevation::Elevated => {
                    def.elevated
                        .as_ref()
                        .ok_or(NetError::NoElevatedVariant(*road))?
                        .cost_per_m
                }
                Elevation::Tunnel => {
                    def.tunnel
                        .as_ref()
                        .ok_or(NetError::NoTunnelVariant(*road))?
                        .cost_per_m
                }
                _ if !def.ground => return Err(NetError::WrongKind(*road)),
                _ => def.cost_per_m,
            };
            let cost = segment_cost(s.length, cost_per_m);
            check_funds(world, cost).map_err(NetError::Funds)?;
            let mut graph = world.resource_mut::<NetGraph>();
            graph.upgrade_segment(*seg, def, *road)?;
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
            let s0 = if *s0 < PART_SNAP { 0.0 } else { *s0 };
            let s1 = if *s1 > len - PART_SNAP { len } else { *s1 };
            if s0 >= s1 {
                return Err(NetError::BadPartRange);
            }
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
        check_plan(&graph, &plan).map_err(NetError::from)?;
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
    check_plan(graph, &plan).map_err(NetError::from)?;
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
    apply_plan(&mut after, content, plan).ok()?;
    Some(after)
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

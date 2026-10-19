//! The "one cell between roads" check of the road tool (`architecture/system-design.md` §6.7,
//! T-124; the player's decision is to forbid it). A pure report over a validated plan: the
//! tool shows a red ghost and sends nothing. `validate_road_path` and the `BuildRoad`
//! application never call it, so a command with such a path is still accepted and every log and
//! checksum is unchanged.

use bevy_math::Vec2;
use cp_content::{ContentDb, RoadDefId};
use cp_core::curve::to_xz;

use super::{PlannedSegment, RoadBuildPlan, ZONE_CELL, dir_xz};
use crate::graph::NetGraph;
use crate::model::{Elevation, SegmentId};

/// Sample spacing along the new piece, metres.
const SAMPLE: f32 = 4.0;
/// A stretch of closeness must be longer than this to count, metres.
const MIN_RUN: f32 = 8.0;
/// Roads meeting at more than this angle (degrees, undirected) are joining or crossing, not
/// running side by side; the shallowest allowed junction is 30°.
const PARALLEL_DEG: f32 = 25.0;
/// Points within this of a centre line count as on it (a joined end).
const ON_AXIS: f32 = 0.5;

/// Part of a new road that runs closer than one cell to an existing road.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClearanceWarning {
    /// Middle of the offending stretch.
    pub pos: Vec2,
    pub other: SegmentId,
}

/// Every stretch of a ground piece of `plan` longer than 8 m whose edge is closer than
/// [`ZONE_CELL`] to the edge of an existing ground segment, leaving out the surroundings
/// (`half_width_a + half_width_b + 8 m`) of the points where the two meet, join or cross.
/// Ordered by the plan's pieces, then by segment id.
pub fn clearance_report(
    graph: &NetGraph,
    content: &ContentDb,
    plan: &RoadBuildPlan,
) -> Vec<ClearanceWarning> {
    side_by_side(graph, content, plan.road, &plan.segments, ZONE_CELL - 1e-3)
}

/// Every stretch of a ground piece longer than 8 m whose asphalt overlaps that of an existing
/// ground segment it runs side by side with (within 25° of parallel, away from the joints): a real
/// collision, which the sim refuses for a literal plan (T-264). The same walk as
/// [`clearance_report`] with the gap at zero.
pub fn overlap_report(
    graph: &NetGraph,
    content: &ContentDb,
    road: RoadDefId,
    segments: &[PlannedSegment],
) -> Vec<ClearanceWarning> {
    side_by_side(graph, content, road, segments, -0.05)
}

fn side_by_side(
    graph: &NetGraph,
    content: &ContentDb,
    road: RoadDefId,
    segments: &[PlannedSegment],
    gap: f32,
) -> Vec<ClearanceWarning> {
    let hw_a = content.road(road).width * 0.5;
    let cos_parallel = PARALLEL_DEG.to_radians().cos();
    let mut out = Vec::new();
    for piece in segments {
        if piece.elevation != Elevation::Ground {
            continue;
        }
        let table = piece.curve.arc_table();
        let len = table.length();
        let n = ((len / SAMPLE).ceil() as usize).max(1);
        let step = len / n as f32;
        let samples: Vec<(Vec2, Vec2)> = (0..=n)
            .map(|k| {
                let t = table.s_to_t(k as f32 * step);
                (to_xz(piece.curve.point(t)), dir_xz(piece.curve.tangent(t)))
            })
            .collect();
        let (lo, hi) = samples
            .iter()
            .fold((samples[0].0, samples[0].0), |(lo, hi), (p, _)| {
                (lo.min(*p), hi.max(*p))
            });
        for (id, other) in graph.segments() {
            if other.elevation != Elevation::Ground {
                continue;
            }
            let hw_b = content.road(other.road).width * 0.5;
            let reach = hw_a + hw_b + ZONE_CELL;
            let centre = (lo + hi) * 0.5;
            if other.curve.bounds_distance_xz(centre) > (hi - lo).length() * 0.5 + reach + 1e-3 {
                continue;
            }
            // Where the two roads meet: ends of one lying on the other, and crossings.
            let mut joints: Vec<Vec2> = Vec::new();
            for end in [piece.curve.p0, piece.curve.p3] {
                if other.curve.nearest_xz(to_xz(end)).1 < ON_AXIS {
                    joints.push(to_xz(end));
                }
            }
            for end in [other.curve.p0, other.curve.p3] {
                if piece.curve.nearest_xz(to_xz(end)).1 < ON_AXIS {
                    joints.push(to_xz(end));
                }
            }
            joints.extend(
                piece
                    .curve
                    .intersect_xz(&other.curve)
                    .into_iter()
                    .map(|(tp, _)| to_xz(piece.curve.point(tp))),
            );
            let mut run: Vec<Vec2> = Vec::new();
            let flush = |run: &mut Vec<Vec2>, out: &mut Vec<ClearanceWarning>| {
                if run.len().saturating_sub(1) as f32 * step > MIN_RUN {
                    out.push(ClearanceWarning {
                        pos: run[run.len() / 2],
                        other: id,
                    });
                }
                run.clear();
            };
            for &(p, dir) in &samples {
                let (t, d) = other.curve.nearest_xz(p);
                // Past the end of the other road its asphalt is a junction's, not a lane beside.
                let beside = gap >= 0.0 || (0.01..0.99).contains(&t);
                let close = beside
                    && d - hw_a - hw_b < gap
                    && dir.dot(dir_xz(other.curve.tangent(t))).abs() > cos_parallel
                    && joints.iter().all(|j| j.distance(p) > reach);
                if close {
                    run.push(p);
                } else {
                    flush(&mut run, &mut out);
                }
            }
            flush(&mut run, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{NodeRef, PlannedSegment, SnapReport};
    use bevy_math::Vec3;
    use cp_core::Cost;
    use cp_core::curve::CubicBezier3;

    fn db() -> ContentDb {
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn p3(x: f32, z: f32) -> Vec3 {
        Vec3::new(x, 0.0, z)
    }

    fn existing(db: &ContentDb, a: Vec3, b: Vec3) -> NetGraph {
        let id = db.road_id("basic_2l").unwrap();
        let mut g = NetGraph::new();
        g.add_segment(db.road(id), id, CubicBezier3::line(a, b), Elevation::Ground)
            .unwrap();
        g
    }

    fn plan(db: &ContentDb, a: Vec3, b: Vec3) -> RoadBuildPlan {
        let road = db.road_id("basic_2l").unwrap();
        let curve = CubicBezier3::line(a, b);
        RoadBuildPlan {
            road,
            segments: vec![PlannedSegment {
                start: NodeRef::New(0),
                end: NodeRef::New(1),
                curve,
                elevation: Elevation::Ground,
                length: curve.arc_table().length(),
                cost: Cost::ZERO,
            }],
            new_nodes: Vec::new(),
            splits: Vec::new(),
            snaps: Vec::new(),
            report: SnapReport::default(),
            demolish: Vec::new(),
            cost: Cost::ZERO,
            literal: false,
            dissolve: Vec::new(),
        }
    }

    #[test]
    fn a_parallel_road_with_6_m_between_the_edges_warns_and_with_8_m_does_not() {
        let db = db();
        let w = db.road(db.road_id("basic_2l").unwrap()).width;
        let g = existing(&db, p3(0.0, 0.0), p3(100.0, 0.0));
        for (gap, warns) in [(6.0, true), (7.5, true), (8.0, false), (20.0, false)] {
            let z = w + gap;
            let r = clearance_report(&g, &db, &plan(&db, p3(0.0, z), p3(100.0, z)));
            assert_eq!(!r.is_empty(), warns, "gap {gap}: {r:?}");
        }
        let r = clearance_report(&g, &db, &plan(&db, p3(0.0, w + 6.0), p3(100.0, w + 6.0)));
        assert_eq!(r.len(), 1);
        assert!((r[0].pos.x - 50.0).abs() < 10.0);
    }

    #[test]
    fn joined_and_crossing_roads_do_not_warn() {
        let db = db();
        let g = existing(&db, p3(0.0, 0.0), p3(100.0, 0.0));
        // A T: ends on the road.
        assert!(clearance_report(&g, &db, &plan(&db, p3(50.0, 0.0), p3(50.0, 80.0))).is_empty());
        // A cross.
        assert!(clearance_report(&g, &db, &plan(&db, p3(50.0, -60.0), p3(50.0, 60.0))).is_empty());
        // A shallow (30 degree) branch from the end node.
        let (c, s) = (30.0_f32.to_radians().cos(), 30.0_f32.to_radians().sin());
        let branch = plan(&db, p3(100.0, 0.0), p3(100.0 + 100.0 * c, 100.0 * s));
        assert!(clearance_report(&g, &db, &branch).is_empty());
        // A continuation of the same road.
        assert!(clearance_report(&g, &db, &plan(&db, p3(100.0, 0.0), p3(200.0, 0.0))).is_empty());
    }

    #[test]
    fn short_closeness_does_not_count() {
        let db = db();
        let w = db.road(db.road_id("basic_2l").unwrap()).width;
        let g = existing(&db, p3(0.0, 0.0), p3(100.0, 0.0));
        // Only 8 m long and 3 m from the edge: not longer than a cell.
        let r = clearance_report(&g, &db, &plan(&db, p3(40.0, w + 3.0), p3(48.0, w + 3.0)));
        assert!(r.is_empty(), "{r:?}");
    }
}

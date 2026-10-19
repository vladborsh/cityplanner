//! The graph invariants of `architecture/system-design.md` §6.8.3 item 2 (T-262).
//!
//! [`NetGraph::check_invariants`] is what the tests and the debug builds ask after a command:
//! the links between nodes and segments hold, no node has more than eight segments, every
//! segment is [`MIN_LEN`]..=[`MAX_LEN`] metres long, and no two ground segments cross without
//! a node. Built by `BuildPlan` the graph keeps all of them; a road laid by the older
//! `BuildRoad` (old logs, scenes) may break the length rule, which is why the debug check
//! after a command only asks that the command made nothing worse.

use cp_core::curve::to_xz;

use super::NetGraph;
use crate::model::{Elevation, MAX_SEGMENTS_PER_NODE, NodeId, SegmentId};

/// Shortest segment, metres.
pub const MIN_LEN: f32 = 8.0;
/// Longest segment, metres.
pub const MAX_LEN: f32 = 100.0;

/// Slack of the length bounds (the arc length of a piece cut at an equal share).
const LEN_EPS: f32 = 0.05;

/// One broken invariant.
#[derive(Clone, Debug, PartialEq)]
pub enum Violation {
    /// A segment shorter than [`MIN_LEN`] or longer than [`MAX_LEN`].
    SegmentLength { seg: SegmentId, length: f32 },
    /// A segment names a node the graph does not have.
    MissingNode { seg: SegmentId, node: NodeId },
    /// A segment and the segment list of its end node disagree.
    Unlinked { seg: SegmentId, node: NodeId },
    /// A node lists a segment that does not exist or does not end in it.
    DanglingLink { node: NodeId, seg: SegmentId },
    /// A node with more than [`MAX_SEGMENTS_PER_NODE`] segments.
    TooManySegments { node: NodeId },
    /// Two ground segments of the same kind cross (or overlap) where there is no node.
    Crossing { a: SegmentId, b: SegmentId },
}

impl NetGraph {
    /// The structural invariants and the length bounds, in `O(segments)`.
    pub fn check_basic_invariants(&self) -> Vec<Violation> {
        let mut out = Vec::new();
        for (id, s) in self.segments() {
            if s.length < MIN_LEN - LEN_EPS || s.length > MAX_LEN + LEN_EPS {
                out.push(Violation::SegmentLength {
                    seg: id,
                    length: s.length,
                });
            }
            for n in [s.start, s.end] {
                match self.node(n) {
                    None => out.push(Violation::MissingNode { seg: id, node: n }),
                    Some(node) if !node.segments.contains(&id) => {
                        out.push(Violation::Unlinked { seg: id, node: n });
                    }
                    Some(_) => {}
                }
            }
        }
        for (id, n) in self.nodes() {
            if n.segments.len() > MAX_SEGMENTS_PER_NODE {
                out.push(Violation::TooManySegments { node: id });
            }
            for &sid in &n.segments {
                if !self
                    .segment(sid)
                    .is_some_and(|s| s.start == id || s.end == id)
                {
                    out.push(Violation::DanglingLink { node: id, seg: sid });
                }
            }
        }
        out
    }

    /// Every invariant: [`Self::check_basic_invariants`] and the crossings. Quadratic in the
    /// segments (pruned by their boxes): for tests, not for every tick.
    pub fn check_invariants(&self) -> Vec<Violation> {
        let mut out = self.check_basic_invariants();
        let ground: Vec<_> = self
            .segments()
            .filter(|(_, s)| s.elevation == Elevation::Ground)
            .map(|(id, s)| {
                let c = [s.curve.p0, s.curve.p1, s.curve.p2, s.curve.p3].map(to_xz);
                let lo = c[0].min(c[1]).min(c[2]).min(c[3]);
                let hi = c[0].max(c[1]).max(c[2]).max(c[3]);
                (id, s, lo, hi)
            })
            .collect();
        for (i, (a, sa, alo, ahi)) in ground.iter().enumerate() {
            for (b, sb, blo, bhi) in &ground[i + 1..] {
                if ahi.x < blo.x || bhi.x < alo.x || ahi.y < blo.y || bhi.y < alo.y {
                    continue;
                }
                let shared = [sa.start, sa.end]
                    .iter()
                    .any(|n| *n == sb.start || *n == sb.end);
                // Segments that meet in a node may touch there; anywhere else they may not.
                let crossing = sa
                    .curve
                    .intersect_xz(&sb.curve)
                    .into_iter()
                    .any(|(ta, tb)| {
                        let near_end = |t: f32| !(0.01..=0.99).contains(&t);
                        !(shared && near_end(ta) && near_end(tb))
                    });
                if crossing {
                    out.push(Violation::Crossing { a: *a, b: *b });
                }
            }
        }
        out
    }
}

//! Where the metro trains are (T-164). A train is an abstract timed run (T-163): the sim keeps
//! the leg it is on and the tick it arrives, not a place. The client draws it by interpolating
//! along the leg of the plan on the track lanes; this is that interpolation, a pure function of
//! the fleet, the runs, the plans and the track graph.

use bevy_math::Vec3;
use cp_core::Tick;
use cp_core::time::MOVE_SECONDS_PER_TICK;
use cp_traffic::{PathGraph, RunState, ServiceRuns};

use crate::fleet::{BusPhase, TransitFleet};
use crate::model::{LineId, TransitMode, TransitNet};
use crate::plan::{LinePlan, LinePlans, visits};

/// Metres between the points of the path a train is placed on.
const POSE_STEP: f32 = 2.0;

/// A train as the client draws it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetroTrain {
    pub line: LineId,
    /// Palette index of its line.
    pub color: u8,
    /// Where it is now, with the height of the track.
    pub pos: Vec3,
    /// Where it is one tick later (the client draws between the two).
    pub next: Vec3,
}

/// The point `f` (`0..=1`) of the way along `points`.
fn along(points: &[Vec3], f: f32) -> Option<Vec3> {
    let (first, last) = (*points.first()?, *points.last()?);
    if points.len() < 2 {
        return Some(first);
    }
    let total: f32 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
    if total <= 1e-3 {
        return Some(first);
    }
    let want = f.clamp(0.0, 1.0) * total;
    let mut done = 0.0;
    for w in points.windows(2) {
        let d = w[0].distance(w[1]);
        if done + d >= want {
            let t = if d > 1e-4 { (want - done) / d } else { 0.0 };
            return Some(w[0].lerp(w[1], t));
        }
        done += d;
    }
    Some(last)
}

/// The leg of `plan` a train in `phase` is on, and how far along it is (`0..=1`) now and a tick
/// later. A train standing at a station is at the end of the leg it came on.
fn progress(
    plan: &LinePlan,
    n: usize,
    phase: BusPhase,
    state: RunState,
    now: Tick,
) -> Option<(usize, f32, f32)> {
    let m = visits(n);
    if plan.legs.len() != m {
        return None;
    }
    let came_on = |idx: u16| (usize::from(idx) + m - 1) % m;
    match phase {
        BusPhase::Leaving => Some((0, 0.0, 0.0)),
        BusPhase::Dwell { idx, .. } => Some((came_on(idx), 1.0, 1.0)),
        BusPhase::Driving { idx } => {
            let leg = came_on(idx);
            let total = ((plan.legs[leg].path.cost / MOVE_SECONDS_PER_TICK).ceil() as u64).max(1);
            let at = |remaining: u64| 1.0 - (remaining as f32 / total as f32).clamp(0.0, 1.0);
            Some(match state {
                RunState::Timer { arrive_at } => {
                    let remaining = arrive_at.0.saturating_sub(now.0);
                    (leg, at(remaining), at(remaining.saturating_sub(1)))
                }
                RunState::Arrived { .. } | RunState::Parked { .. } => (leg, 1.0, 1.0),
                _ => (leg, 0.0, 0.0),
            })
        }
        BusPhase::ToDepot => None,
    }
}

/// The trains of the metro lines now, in fleet (id) order. A line whose plan is not whole has
/// none drawn.
pub fn metro_trains(
    transit: &TransitNet,
    plans: &LinePlans,
    fleet: &TransitFleet,
    runs: &ServiceRuns,
    graph: &PathGraph,
    now: Tick,
) -> Vec<MetroTrain> {
    let mut out = Vec::new();
    for (_, bus) in fleet.buses.iter() {
        let Some(line) = transit
            .lines
            .get(bus.line)
            .filter(|l| l.mode == TransitMode::Metro)
        else {
            continue;
        };
        let (Some(plan), Some(run)) =
            (plans.get(bus.line).filter(|p| p.is_ok()), runs.get(bus.run))
        else {
            continue;
        };
        let Some((leg, f0, f1)) = progress(plan, line.stops.len(), bus.phase, run.state, now)
        else {
            continue;
        };
        let points = graph.rail_path_points3(&plan.legs[leg].path, POSE_STEP);
        let (Some(pos), Some(next)) = (along(&points, f0), along(&points, f1)) else {
            continue;
        };
        out.push(MetroTrain {
            line: bus.line,
            color: line.color,
            pos,
            next,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn along_walks_a_polyline_by_length() {
        let pts = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 30.0),
        ];
        assert_eq!(along(&pts, 0.0), Some(pts[0]));
        assert_eq!(along(&pts, 1.0), Some(pts[2]));
        // 40 m long: a quarter is the corner.
        let q = along(&pts, 0.25).unwrap();
        assert!(q.distance(pts[1]) < 1e-4, "{q:?}");
        assert!(along(&[], 0.5).is_none());
    }
}

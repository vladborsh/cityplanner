//! Where the metro trains are (T-164). A train is an abstract timed run (T-163): the sim keeps
//! the leg it is on and the tick it arrives, not a place. The client draws it by interpolating
//! along the leg of the plan on the track lanes; this is that interpolation, a pure function of
//! the fleet, the runs, the plans and the track graph.

use bevy_math::Vec3;
use cp_core::Tick;
use cp_core::time::MOVE_SECONDS_PER_TICK;
use cp_traffic::{PathGraph, RunId, RunPoint, RunState, ServiceRuns, find_rail_path};

use crate::fleet::{BusPhase, TransitFleet};
use crate::intercity::Intercity;
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

/// The intercity train as the client draws it (T-167): the polyline of the leg it is on (the
/// ride in from the edge, or the ride out) with the height of the track, and how far along it
/// the head is now and a tick later, in metres. The cars stand behind the head on the same line.
#[derive(Clone, Debug, PartialEq)]
pub struct IntercityPose {
    pub points: Vec<Vec3>,
    pub s: f32,
    pub next_s: f32,
}

impl IntercityPose {
    pub fn length(&self) -> f32 {
        self.points.windows(2).map(|w| w[0].distance(w[1])).sum()
    }

    /// The point and unit tangent at `s` metres along the line (clamped to it).
    pub fn at(&self, s: f32) -> (Vec3, Vec3) {
        let mut left = s.max(0.0);
        let mut last = (self.points.first().copied().unwrap_or_default(), Vec3::Z);
        for w in self.points.windows(2) {
            let d = w[0].distance(w[1]);
            let dir = (w[1] - w[0]).try_normalize().unwrap_or(Vec3::Z);
            last = (w[1], dir);
            if left <= d {
                return (w[0] + dir * left, dir);
            }
            left -= d;
        }
        last
    }
}

/// Where the intercity train is, if one is on the line: the leg of its run on the track and its
/// progress along it (a train that stands at the station is at the end of the ride in).
pub fn intercity_pose(
    intercity: &Intercity,
    runs: &ServiceRuns,
    graph: &PathGraph,
    now: Tick,
) -> Option<IntercityPose> {
    run_pose(intercity.train?.run, runs, graph, now)
}

/// Where the train of run `id` is on the leg of its run (T-216b: the cargo trains use it too).
pub fn run_pose(
    id: RunId,
    runs: &ServiceRuns,
    graph: &PathGraph,
    now: Tick,
) -> Option<IntercityPose> {
    let run = runs.get(id)?;
    let (RunPoint::Access(from), RunPoint::Access(to)) = (run.from, run.to) else {
        return None;
    };
    let path = find_rail_path(graph, &from, &to).ok()?;
    let points = graph.rail_path_points3(&path, POSE_STEP);
    if points.len() < 2 {
        return None;
    }
    let total = ((path.cost / MOVE_SECONDS_PER_TICK).ceil() as u64).max(1);
    let at = |remaining: u64| 1.0 - (remaining as f32 / total as f32).clamp(0.0, 1.0);
    let (f0, f1) = match run.state {
        RunState::Timer { arrive_at } => {
            let remaining = arrive_at.0.saturating_sub(now.0);
            (at(remaining), at(remaining.saturating_sub(1)))
        }
        RunState::Arrived { .. } | RunState::Parked { .. } => (1.0, 1.0),
        _ => (0.0, 0.0),
    };
    let mut pose = IntercityPose {
        points,
        s: 0.0,
        next_s: 0.0,
    };
    let len = pose.length();
    (pose.s, pose.next_s) = (f0 * len, f1 * len);
    Some(pose)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_intercity_pose_walks_its_line_by_metres() {
        let pose = IntercityPose {
            points: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 30.0),
            ],
            s: 0.0,
            next_s: 0.0,
        };
        assert_eq!(pose.length(), 40.0);
        let (p, d) = pose.at(5.0);
        assert!(p.distance(Vec3::new(5.0, 0.0, 0.0)) < 1e-4 && d == Vec3::X);
        let (p, d) = pose.at(25.0);
        assert!(p.distance(Vec3::new(10.0, 0.0, 15.0)) < 1e-4 && d == Vec3::Z);
        // Before the start and past the end it stays on the line.
        assert_eq!(pose.at(-3.0).0, Vec3::ZERO);
        assert_eq!(pose.at(99.0).0, Vec3::new(10.0, 0.0, 30.0));
    }

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

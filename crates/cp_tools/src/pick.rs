//! Picking for the info panels (T-044): what the cursor points at when no tool is active.
//!
//! **Priority**: a vehicle or pedestrian under the cursor wins over a building, a building over
//! a road segment. Agents are compared with where the renderer draws them (the pose
//! interpolated by the tick fraction, [`cp_render::agent::vehicle_positions`]), within a reach
//! that grows with the camera height so they stay clickable from above; the nearest one wins
//! (a vehicle beats a pedestrian only when it is nearer, in units of its own reach). Agents the
//! renderer does not draw at that height are not pickable either.

use bevy::prelude::*;
use bevy_math::Vec2;
use cp_render::agent::{
    PEDESTRIAN_MAX_HEIGHT, VEHICLE_MAX_HEIGHT, pedestrian_positions, vehicle_positions,
};
use cp_sim::{Selection, SimView};

use crate::building::pick_building;
use crate::bulldozer::{PICK_RADIUS, pick_segment};

/// Reach around a vehicle's centre at ground level, metres (a car is about 4 m long).
const VEHICLE_REACH: f32 = 3.5;
/// Reach around a pedestrian, metres.
const PEDESTRIAN_REACH: f32 = 1.2;
/// Extra reach per metre of camera height.
const REACH_PER_HEIGHT: f32 = 0.012;

/// The thing under the cursor at `p` (terrain point), seen from `camera_height` metres above
/// the ground, with the agents drawn `alpha` of the way between their last two poses.
pub fn pick(view: &SimView, alpha: f32, p: Vec2, camera_height: f32) -> Option<Selection> {
    pick_with(view, alpha, p, camera_height, false)
}

/// How far from a line's ribbon a click still means the line, metres.
pub const RIBBON_PICK_RADIUS: f32 = 6.0;

/// The transit line under `p`: one that calls at a stop within reach, else the one whose
/// ribbon passes within [`RIBBON_PICK_RADIUS`] (the nearer wins).
pub fn pick_line(view: &SimView, p: Vec2) -> Option<cp_transit::LineId> {
    let stop = view
        .transit
        .net
        .stops
        .iter()
        .map(|(_, st)| (st.pos.distance(p), st))
        .filter(|(d, st)| *d <= crate::transit::STOP_PICK_RADIUS && !st.lines.is_empty())
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, st)) = stop {
        return st.lines.first().copied();
    }
    let mut best: Option<(f32, cp_transit::LineId)> = None;
    for (&id, route) in &view.transit.routes {
        for w in route.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = if ab.length_squared() > 0.0 {
                ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let d = (a + ab * t).distance(p);
            if d <= RIBBON_PICK_RADIUS && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, id));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// [`pick`] that also finds transit lines (stops before buildings, ribbons before roads) while
/// `lines` is set: the Transit view is showing them.
pub fn pick_with(
    view: &SimView,
    alpha: f32,
    p: Vec2,
    camera_height: f32,
    lines: bool,
) -> Option<Selection> {
    let grow = camera_height.max(0.0) * REACH_PER_HEIGHT;
    let mut best: Option<(f32, Selection)> = None;
    let mut consider = |score: f32, sel: Selection| {
        if score <= 1.0 && best.is_none_or(|(s, _)| score < s) {
            best = Some((score, sel));
        }
    };
    if camera_height <= VEHICLE_MAX_HEIGHT {
        let reach = VEHICLE_REACH + grow;
        for (key, pos) in
            vehicle_positions(&view.agents.prev.vehicles, &view.agents.cur.vehicles, alpha)
        {
            consider(
                Vec2::new(pos.x, pos.z).distance(p) / reach,
                Selection::Vehicle(key),
            );
        }
    }
    if camera_height <= PEDESTRIAN_MAX_HEIGHT {
        let reach = PEDESTRIAN_REACH + grow;
        for (key, pos) in pedestrian_positions(
            &view.agents.prev.pedestrians,
            &view.agents.cur.pedestrians,
            alpha,
        ) {
            consider(pos.distance(p) / reach, Selection::Pedestrian(key));
        }
    }
    if let Some((_, sel)) = best {
        return Some(sel);
    }
    if lines
        && let Some(st) = view
            .transit
            .net
            .stops
            .iter()
            .find(|(_, st)| {
                !st.lines.is_empty() && st.pos.distance(p) <= crate::transit::STOP_PICK_RADIUS
            })
            .map(|(_, st)| st)
        && let Some(&line) = st.lines.first()
    {
        return Some(Selection::Line(line));
    }
    if let Some(id) = pick_building(view, p) {
        return Some(Selection::Building(id));
    }
    if lines && let Some(line) = pick_line(view, p) {
        return Some(Selection::Line(line));
    }
    pick_segment(&view.net, p, PICK_RADIUS).map(Selection::Segment)
}

/// What is selected (set by a click with no tool active; the app reads it every frame).
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct Picked(pub Option<Selection>);

/// The details of [`Picked`], refreshed by the app every frame from the sim.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
pub struct Inspected(pub Option<cp_sim::Inspection>);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use bevy_math::Vec3;
    use cp_sim::{AgentFrame, AgentSnapshot, PedestrianPose, VehiclePose};

    fn view_with(
        vehicles: Vec<VehiclePose>,
        peds: Vec<PedestrianPose>,
        prev: AgentFrame,
    ) -> SimView {
        let sim = cp_sim::SimRunner::new(1);
        let mut v = sim.view();
        v.agents = Arc::new(AgentSnapshot {
            tick: v.tick,
            prev: Arc::new(prev),
            cur: Arc::new(AgentFrame {
                vehicles,
                pedestrians: peds,
            }),
        });
        v
    }

    fn car(key: u64, x: f32, z: f32) -> VehiclePose {
        VehiclePose {
            key,
            kind: 0,
            pos: Vec3::new(x, 0.0, z),
            yaw: 0.0,
            speed: 5.0,
            tint: None,
        }
    }

    fn walker(key: u64, x: f32, z: f32) -> PedestrianPose {
        PedestrianPose {
            key,
            pos: Vec2::new(x, z),
        }
    }

    #[test]
    fn agents_are_picked_where_they_are_drawn_and_beat_whatever_is_below() {
        let prev = AgentFrame {
            vehicles: vec![car(1, 100.0, 100.0)],
            pedestrians: vec![walker(7, 200.0, 200.0)],
        };
        let v = view_with(
            vec![car(1, 110.0, 100.0)],
            vec![walker(7, 202.0, 200.0)],
            prev,
        );
        // Halfway between the poses: x = 105 for the car.
        assert_eq!(
            pick(&v, 0.5, Vec2::new(105.0, 100.5), 20.0),
            Some(Selection::Vehicle(1))
        );
        // Where the car was a moment ago it is no longer.
        assert_eq!(pick(&v, 0.5, Vec2::new(100.0, 100.0), 20.0), None);
        // At the start of the tick it is still there.
        assert_eq!(
            pick(&v, 0.0, Vec2::new(100.0, 100.0), 20.0),
            Some(Selection::Vehicle(1))
        );
        assert_eq!(
            pick(&v, 1.0, Vec2::new(202.0, 200.5), 20.0),
            Some(Selection::Pedestrian(7))
        );
        // Nothing near: nothing (an empty map has no buildings or roads).
        assert_eq!(pick(&v, 0.5, Vec2::new(500.0, 500.0), 20.0), None);
    }

    #[test]
    fn reach_grows_with_height_and_agents_vanish_above_the_draw_limits() {
        let v = view_with(
            vec![car(1, 100.0, 100.0)],
            vec![walker(7, 300.0, 300.0)],
            AgentFrame::default(),
        );
        let off = Vec2::new(100.0 + 6.0, 100.0);
        assert_eq!(pick(&v, 1.0, off, 20.0), None, "6 m off at street level");
        assert_eq!(
            pick(&v, 1.0, off, 300.0),
            Some(Selection::Vehicle(1)),
            "from 300 m up"
        );
        // Pedestrians are not drawn above 250 m, so they cannot be picked there.
        assert_eq!(
            pick(&v, 1.0, Vec2::new(300.0, 300.0), 100.0),
            Some(Selection::Pedestrian(7))
        );
        assert_eq!(pick(&v, 1.0, Vec2::new(300.0, 300.0), 400.0), None);
        // Nor vehicles above 800 m.
        assert_eq!(pick(&v, 1.0, Vec2::new(100.0, 100.0), 900.0), None);
    }

    #[test]
    fn the_nearer_agent_wins() {
        let v = view_with(
            vec![car(1, 100.0, 100.0)],
            vec![walker(7, 100.8, 100.0)],
            AgentFrame::default(),
        );
        assert_eq!(
            pick(&v, 1.0, Vec2::new(100.7, 100.0), 10.0),
            Some(Selection::Pedestrian(7))
        );
        assert_eq!(
            pick(&v, 1.0, Vec2::new(99.0, 100.0), 10.0),
            Some(Selection::Vehicle(1))
        );
    }
}

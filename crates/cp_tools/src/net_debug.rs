//! Debug net overlay (T-025, `system-design.md`): gizmos for the road network — the lanes with
//! arrows for their direction, the lane connections of each junction (coloured by turn), the
//! conflicting pairs of the junction under the cursor and the phase of a traffic light as the
//! colour of its connections. Four switches in the debug window (F3); with all of them off the
//! system draws nothing.

use bevy::prelude::*;
use cp_net::{Signal, Turn};
use cp_sim::SimView;

use crate::plugin::ToolState;

/// Junctions farther than this from the cursor are not drawn, metres.
pub const DRAW_RADIUS_M: f32 = 160.0;
/// The conflicts are shown for the junction within this of the cursor, metres.
pub const HOVER_NODE_M: f32 = 40.0;
/// Height above the ground the lines are drawn at, metres.
const LIFT: f32 = 0.35;

/// The switches of the overlay. All off by default.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetDebug {
    /// Lane centre lines with an arrow in the direction of travel.
    pub lanes: bool,
    /// The connections through junctions, coloured by turn.
    pub connections: bool,
    /// The conflicting pairs of the junction under the cursor.
    pub conflicts: bool,
    /// The connections of lights coloured by their signal now.
    pub signals: bool,
}

impl NetDebug {
    pub fn any(&self) -> bool {
        self.lanes || self.connections || self.conflicts || self.signals
    }
}

/// One line of the overlay, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugLine {
    pub a: Vec3,
    pub b: Vec3,
    pub color: [f32; 3],
}

const LANE: [f32; 3] = [0.75, 0.75, 0.8];
const CONFLICT: [f32; 3] = [0.95, 0.15, 0.15];

fn turn_color(t: Turn) -> [f32; 3] {
    match t {
        Turn::Left => [0.25, 0.55, 1.0],
        Turn::Straight => [0.95, 0.95, 0.95],
        Turn::Right => [1.0, 0.65, 0.2],
        Turn::UTurn => [0.8, 0.3, 0.9],
    }
}

fn signal_color(s: Signal) -> [f32; 3] {
    match s {
        Signal::Green => [0.2, 0.9, 0.3],
        Signal::Yellow => [1.0, 0.85, 0.2],
        Signal::Red => [0.95, 0.2, 0.2],
    }
}

/// An arrow head at `tip` pointing along `dir` (unit, XZ-ish).
fn arrow(out: &mut Vec<DebugLine>, tip: Vec3, dir: Vec3, color: [f32; 3]) {
    let dir = dir.try_normalize().unwrap_or(Vec3::Z);
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    for s in [-1.0, 1.0] {
        out.push(DebugLine {
            a: tip,
            b: tip - dir * 1.4 + side * 0.7 * s,
            color,
        });
    }
}

fn curve_lines(out: &mut Vec<DebugLine>, c: &cp_core::curve::CubicBezier3, color: [f32; 3]) {
    const N: usize = 8;
    let at = |i: usize| c.point(i as f32 / N as f32) + Vec3::Y * LIFT;
    for i in 0..N {
        out.push(DebugLine {
            a: at(i),
            b: at(i + 1),
            color,
        });
    }
    arrow(out, at(N), c.tangent(1.0), color);
}

/// The lines of the overlay for `view` with `cursor` under the mouse (XZ). Empty when every
/// switch is off.
pub fn net_debug_lines(view: &SimView, flags: NetDebug, cursor: Option<Vec2>) -> Vec<DebugLine> {
    let mut out = Vec::new();
    if !flags.any() {
        return out;
    }
    let near = |p: Vec2| cursor.is_none_or(|c| c.distance(p) <= DRAW_RADIUS_M);
    if flags.lanes {
        for (_, lane) in view.net.lanes() {
            let mid = lane.curve[lane.curve.len() / 2].point(0.0);
            if !near(Vec2::new(mid.x, mid.z)) {
                continue;
            }
            for piece in &lane.curve {
                curve_lines(&mut out, piece, LANE);
            }
        }
    }
    let hovered = cursor.and_then(|c| {
        view.junctions
            .iter()
            .filter(|(_, j)| Vec2::new(j.pos.x, j.pos.z).distance(c) <= HOVER_NODE_M)
            .min_by(|a, b| {
                let d = |j: &cp_net::Junction| Vec2::new(j.pos.x, j.pos.z).distance(c);
                d(a.1).total_cmp(&d(b.1)).then(a.0.cmp(b.0))
            })
            .map(|(&id, _)| id)
    });
    for (&id, j) in view.junctions.iter() {
        if !near(Vec2::new(j.pos.x, j.pos.z)) {
            continue;
        }
        if flags.connections || flags.signals {
            for (i, c) in j.links.connections.iter().enumerate() {
                let signal = if flags.signals {
                    j.control
                        .light
                        .as_ref()
                        .map(|l| l.connection_signal(i as u16, view.tick.0))
                } else {
                    None
                };
                let color = match signal {
                    Some(s) => signal_color(s),
                    None if flags.connections => turn_color(c.turn),
                    None => continue,
                };
                curve_lines(&mut out, &c.curve, color);
            }
        }
        if flags.conflicts && hovered == Some(id) {
            let mid = |i: u16| {
                let c = &j.links.connections[usize::from(i)];
                c.curve.point(0.5) + Vec3::Y * (LIFT + 0.3)
            };
            for &(a, b) in &j.links.conflicts {
                out.push(DebugLine {
                    a: mid(a),
                    b: mid(b),
                    color: CONFLICT,
                });
            }
        }
    }
    out
}

/// `Update`: draws the overlay (also in a run without the tools, e.g. `--hidden`).
pub fn draw_net_debug(
    flags: Res<NetDebug>,
    view: Option<Res<SimView>>,
    state: Option<Res<ToolState>>,
    mut gizmos: Gizmos,
) {
    if !flags.any() {
        return;
    }
    let Some(view) = view else { return };
    for l in net_debug_lines(&view, *flags, state.and_then(|s| s.hover)) {
        gizmos.line(l.a, l.b, Color::srgb(l.color[0], l.color[1], l.color[2]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, NetGraph};
    use cp_sim::SimRunner;
    use std::sync::Arc;

    /// A junction of `arms` roads (`basic_2l`, or `wide` for 6 lanes) of 100 m around (0, 0).
    fn junction_view(arms: &[(f32, f32)], road: &str) -> SimView {
        let db = Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        );
        let mut sim = SimRunner::new(1);
        sim.set_content(Arc::clone(&db));
        let id = db.road_id(road).unwrap();
        let def = db.road(id).clone();
        for &(x, z) in arms {
            sim.world_mut()
                .resource_mut::<NetGraph>()
                .add_segment(
                    &def,
                    id,
                    CubicBezier3::line(Vec3::ZERO, Vec3::new(x, 0.0, z)),
                    Elevation::Ground,
                )
                .unwrap();
        }
        sim.tick_once();
        sim.tick_once();
        sim.view()
    }

    fn on(f: impl FnOnce(&mut NetDebug)) -> NetDebug {
        let mut n = NetDebug::default();
        f(&mut n);
        n
    }

    #[test]
    fn the_overlay_draws_nothing_when_it_is_off() {
        let v = junction_view(&[(100.0, 0.0), (-100.0, 0.0), (0.0, 100.0)], "basic_2l");
        assert!(net_debug_lines(&v, NetDebug::default(), None).is_empty());
        assert!(net_debug_lines(&v, NetDebug::default(), Some(Vec2::ZERO)).is_empty());
    }

    #[test]
    fn every_connection_of_t_x_and_six_lane_junctions_is_drawn() {
        for (arms, road) in [
            (vec![(100.0, 0.0), (-100.0, 0.0), (0.0, 100.0)], "basic_2l"),
            (
                vec![(100.0, 0.0), (-100.0, 0.0), (0.0, 100.0), (0.0, -100.0)],
                "basic_2l",
            ),
            (
                vec![(100.0, 0.0), (-100.0, 0.0), (0.0, 100.0), (0.0, -100.0)],
                "large_6l",
            ),
        ] {
            let v = junction_view(&arms, road);
            let (_, j) = v.junctions.iter().find(|(_, j)| j.arms.len() > 2).unwrap();
            let n = j.links.connections.len();
            assert!(n >= 6, "{road}: {n} connections");
            let lines = net_debug_lines(&v, on(|f| f.connections = true), Some(Vec2::ZERO));
            // 8 pieces and two arrow strokes per connection.
            // (The dead ends of the arms are junctions too.)
            let all: usize = v
                .junctions
                .iter()
                .map(|(_, j)| j.links.connections.len())
                .sum();
            assert_eq!(lines.len(), all * 10, "{road}");
            // The colours: all turns of the junction appear.
            let colours: std::collections::BTreeSet<_> = lines
                .iter()
                .map(|l| l.color.map(|c| (c * 100.0) as i32))
                .collect();
            assert!(colours.len() >= 2);
        }
    }

    #[test]
    fn lanes_and_conflicts_follow_their_switches() {
        let v = junction_view(
            &[(100.0, 0.0), (-100.0, 0.0), (0.0, 100.0), (0.0, -100.0)],
            "basic_2l",
        );
        let lanes = net_debug_lines(&v, on(|f| f.lanes = true), None);
        assert!(!lanes.is_empty());
        let conflicts = net_debug_lines(&v, on(|f| f.conflicts = true), Some(Vec2::ZERO));
        let (_, j) = v.junctions.iter().find(|(_, j)| j.arms.len() > 2).unwrap();
        assert_eq!(conflicts.len(), j.links.conflicts.len());
        assert!(conflicts.iter().all(|l| l.color == CONFLICT));
        // Away from any junction the conflicts are not shown.
        assert!(
            net_debug_lines(&v, on(|f| f.conflicts = true), Some(Vec2::new(70.0, 0.0))).is_empty()
        );
    }

    #[test]
    fn a_light_colours_its_connections_by_phase() {
        let v = junction_view(
            &[(100.0, 0.0), (-100.0, 0.0), (0.0, 100.0), (0.0, -100.0)],
            "large_6l",
        );
        let (_, j) = v.junctions.iter().find(|(_, j)| j.arms.len() > 2).unwrap();
        assert!(j.control.light.is_some(), "a 6-lane crossing gets a light");
        let lines = net_debug_lines(&v, on(|f| f.signals = true), Some(Vec2::ZERO));
        assert!(!lines.is_empty());
        let green = signal_color(Signal::Green);
        let red = signal_color(Signal::Red);
        assert!(lines.iter().any(|l| l.color == green));
        assert!(lines.iter().any(|l| l.color == red));
    }
}

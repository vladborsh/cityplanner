//! Connectors (`architecture/system-design.md` §18.2 item 6, T-313b): a schematic of the junction
//! the Lanes mode has chosen, every inbound lane a dot at the end of its arm, every outbound lane
//! another; dragging from an inbound dot to an outbound one adds that connection (a rule with an
//! explicit `Exit.lane`), a click on a drawn line takes it away. The schematic and the commands
//! are plain functions of the net, the window only draws them.

use bevy::math::Vec2;
use bevy_egui::egui;
use cp_content::LaneDir;
use cp_locale::tr;
use cp_net::{Exit, JunctionGeometry, NetCommand, NetGraph, NodeId, SegmentId};
use cp_sim::{SimCommand, SimView};
use cp_tools::{CommandOutbox, ToolState};

/// Distance of the lane dots from the centre of the schematic, schematic units.
const ARM_R: f32 = 60.0;
/// Schematic units per metre across the arm.
const LATERAL: f32 = 2.0;
/// Screen points per schematic unit.
const SCALE: f32 = 2.4;
const DOT_R: f32 = 5.0;
/// A click this close (points) to a connection line picks it.
const LINE_PICK: f32 = 5.0;

/// A lane end in the schematic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot {
    pub segment: SegmentId,
    /// Index of the lane in its segment (the key of the rule and of an exit).
    pub index: u8,
    /// Schematic position relative to the node.
    pub at: Vec2,
}

/// The inbound and outbound lane ends of a node and the connections between them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schematic {
    pub ins: Vec<Dot>,
    pub outs: Vec<Dot>,
    /// `(index in ins, index in outs)`, in the order of the links.
    pub links: Vec<(usize, usize)>,
}

/// The schematic of the junction `node`.
pub fn schematic(net: &NetGraph, junctions: &JunctionGeometry, node: NodeId) -> Schematic {
    let mut out = Schematic::default();
    let Some(j) = junctions.get(node) else {
        return out;
    };
    let centre = j.pos;
    let place = |a: &cp_net::Arm, p: bevy::math::Vec3| {
        let dir = Vec2::new(a.angle.cos(), a.angle.sin());
        let perp = Vec2::new(-dir.y, dir.x);
        let lateral = (Vec2::new(p.x, p.z) - Vec2::new(centre.x, centre.z)).dot(perp);
        dir * ARM_R + perp * lateral * LATERAL
    };
    let mut ids_in = Vec::new();
    let mut ids_out = Vec::new();
    for a in &j.arms {
        let Some(seg) = net.segment(a.segment) else {
            continue;
        };
        for &id in &seg.lanes {
            let Some(l) = net.lane(id) else { continue };
            if !l.kind.is_road_vehicle() {
                continue;
            }
            let inbound = (l.dir == LaneDir::Forward) != a.at_start;
            let dot = |p| Dot {
                segment: a.segment,
                index: l.index,
                at: place(a, p),
            };
            if inbound {
                out.ins.push(dot(l.curve[l.curve.len() - 1].p3));
                ids_in.push(id);
            } else {
                out.outs.push(dot(l.curve[0].p0));
                ids_out.push(id);
            }
        }
    }
    for c in &j.links.connections {
        if let (Some(i), Some(o)) = (
            ids_in.iter().position(|&l| l == c.from),
            ids_out.iter().position(|&l| l == c.to),
        ) {
            out.links.push((i, o));
        }
    }
    out
}

/// The explicit exits of the inbound dot `i` as they stand now.
fn exits_of(s: &Schematic, i: usize) -> Vec<Exit> {
    let mut v: Vec<Exit> = s
        .links
        .iter()
        .filter(|l| l.0 == i)
        .map(|l| Exit {
            arm: s.outs[l.1].segment,
            lane: Some(s.outs[l.1].index),
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

fn rule(node: NodeId, d: &Dot, exits: Vec<Exit>) -> NetCommand {
    NetCommand::SetLaneRule {
        node,
        segment: d.segment,
        lane: d.index,
        exits: Some(exits),
    }
}

/// The command that joins inbound dot `i` to outbound dot `o`: the lane's exits, all explicit,
/// plus that one. `None` when it is there already.
pub fn connect_command(s: &Schematic, node: NodeId, i: usize, o: usize) -> Option<NetCommand> {
    let mut exits = exits_of(s, i);
    let new = Exit {
        arm: s.outs[o].segment,
        lane: Some(s.outs[o].index),
    };
    if exits.contains(&new) {
        return None;
    }
    exits.push(new);
    exits.sort();
    Some(rule(node, &s.ins[i], exits))
}

/// The command that takes the connection `link` (an index into `links`) away. `None` for the last
/// connection of its lane (a lane keeps at least one).
pub fn disconnect_command(s: &Schematic, node: NodeId, link: usize) -> Option<NetCommand> {
    let (i, o) = *s.links.get(link)?;
    let gone = Exit {
        arm: s.outs[o].segment,
        lane: Some(s.outs[o].index),
    };
    let exits: Vec<Exit> = exits_of(s, i).into_iter().filter(|e| *e != gone).collect();
    (!exits.is_empty()).then(|| rule(node, &s.ins[i], exits))
}

/// The connection line nearest to `p` (schematic units) within `reach`.
pub fn pick_link(s: &Schematic, p: Vec2, reach: f32) -> Option<usize> {
    s.links
        .iter()
        .enumerate()
        .map(|(k, &(i, o))| (k, seg_distance(p, s.ins[i].at, s.outs[o].at)))
        .filter(|(_, d)| *d <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(k, _)| k)
}

fn seg_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 {
        ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// The nearest dot of `dots` to `p` within `reach`.
pub fn pick_dot(dots: &[Dot], p: Vec2, reach: f32) -> Option<usize> {
    dots.iter()
        .enumerate()
        .filter(|(_, d)| d.at.distance(p) <= reach)
        .min_by(|a, b| a.1.at.distance(p).total_cmp(&b.1.at.distance(p)))
        .map(|(k, _)| k)
}

/// The Connectors window of the junction the Lanes mode has an arm of: drag from an inbound
/// dot to an outbound one to connect, click a line to cut it.
pub fn connectors_window(
    ctx: &egui::Context,
    view: &SimView,
    tools: &ToolState,
    outbox: &mut CommandOutbox,
    open: &mut bool,
) {
    let Some((node, _)) = tools.lane_arm else {
        return;
    };
    let s = schematic(&view.net, &view.junctions, node);
    if s.ins.is_empty() {
        return;
    }
    let side = (ARM_R * 2.0 + 40.0) * SCALE;
    egui::Window::new(tr("Connectors"))
        .id(egui::Id::new("connectors"))
        .anchor(egui::Align2::LEFT_CENTER, egui::vec2(12.0, 0.0))
        .resizable(false)
        .collapsible(false)
        .open(open)
        .show(ctx, |ui| {
            ui.weak(tr(
                "Drag from a lane going in to a lane going out; click a line to cut it",
            ));
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(side, side), egui::Sense::click_and_drag());
            let mid = resp.rect.center();
            let to_screen = |p: Vec2| mid + egui::vec2(p.x, p.y) * SCALE;
            let to_schem = |p: egui::Pos2| Vec2::new(p.x - mid.x, p.y - mid.y) / SCALE;
            let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
            let reach = DOT_R * 1.6 / SCALE;
            // The drag starts on an inbound dot.
            let id = egui::Id::new("connector_drag");
            if resp.drag_started()
                && let Some(i) = pointer.and_then(|p| pick_dot(&s.ins, to_schem(p), reach))
            {
                ui.data_mut(|d| d.insert_temp(id, i));
            }
            let from: Option<usize> = ui.data(|d| d.get_temp(id));
            if resp.drag_stopped() {
                if let (Some(i), Some(p)) = (from, pointer)
                    && let Some(o) = pick_dot(&s.outs, to_schem(p), reach * 1.5)
                    && let Some(c) = connect_command(&s, node, i, o)
                {
                    outbox.push(SimCommand::Net(c));
                }
                ui.data_mut(|d| d.remove::<usize>(id));
            } else if resp.clicked()
                && let Some(p) = pointer
                && let Some(k) = pick_link(&s, to_schem(p), LINE_PICK / SCALE)
                && let Some(c) = disconnect_command(&s, node, k)
            {
                outbox.push(SimCommand::Net(c));
            }
            let hover_line = pointer
                .filter(|_| from.is_none())
                .and_then(|p| pick_link(&s, to_schem(p), LINE_PICK / SCALE));
            let line = egui::Color32::from_rgb(240, 200, 80);
            for (k, &(i, o)) in s.links.iter().enumerate() {
                let (a, b) = (to_screen(s.ins[i].at), to_screen(s.outs[o].at));
                let c = to_screen(Vec2::ZERO);
                let curve = egui::epaint::QuadraticBezierShape::from_points_stroke(
                    [a, c.lerp(a.lerp(b, 0.5), 0.5), b],
                    false,
                    egui::Color32::TRANSPARENT,
                    egui::Stroke::new(
                        if hover_line == Some(k) { 3.5 } else { 2.0 },
                        if hover_line == Some(k) {
                            egui::Color32::WHITE
                        } else {
                            line
                        },
                    ),
                );
                painter.add(curve);
            }
            if let (Some(i), Some(p)) = (from, pointer) {
                painter.line_segment(
                    [to_screen(s.ins[i].at), p],
                    egui::Stroke::new(2.0, egui::Color32::WHITE),
                );
            }
            for d in &s.ins {
                painter.circle_filled(
                    to_screen(d.at),
                    DOT_R,
                    egui::Color32::from_rgb(110, 180, 240),
                );
            }
            for d in &s.outs {
                painter.circle_filled(
                    to_screen(d.at),
                    DOT_R,
                    egui::Color32::from_rgb(120, 210, 130),
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;
    use bevy::math::Vec3;
    use cp_content::{ContentDb, SimContent};
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, update_junctions};
    use std::sync::Arc;

    const C: Vec3 = Vec3::new(1000.0, 0.0, 1000.0);

    fn cross() -> (World, Vec<SegmentId>, NodeId) {
        let db = Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        );
        let mut g = NetGraph::new();
        let road = db.road_id("medium_4l").unwrap();
        let mut segs = Vec::new();
        for deg in [0.0f32, 90.0, 180.0, 270.0] {
            let a = deg.to_radians();
            let tip = C + Vec3::new(150.0 * a.cos(), 0.0, 150.0 * a.sin());
            segs.push(
                g.add_segment(
                    db.road(road),
                    road,
                    CubicBezier3::line(C, tip),
                    Elevation::Ground,
                )
                .unwrap(),
            );
        }
        let node = g.find_node(C).unwrap();
        let mut world = World::new();
        world.insert_resource(g);
        world.init_resource::<JunctionGeometry>();
        world.insert_resource(SimContent(db));
        world.run_system_cached(update_junctions).unwrap();
        (world, segs, node)
    }

    fn schem(world: &World, node: NodeId) -> Schematic {
        schematic(
            world.resource::<NetGraph>(),
            world.resource::<JunctionGeometry>(),
            node,
        )
    }

    fn apply(world: &mut World, c: NetCommand) {
        let NetCommand::SetLaneRule {
            node,
            segment,
            lane,
            exits,
        } = c
        else {
            panic!("a lane rule")
        };
        world
            .resource_mut::<NetGraph>()
            .set_lane_rule(node, segment, lane, exits)
            .expect("accepted");
        world.run_system_cached(update_junctions).unwrap();
    }

    #[test]
    fn the_schematic_has_a_dot_per_lane_end_and_a_line_per_connection() {
        let (world, _, node) = cross();
        let s = schem(&world, node);
        // Four arms of two lanes each way.
        assert_eq!((s.ins.len(), s.outs.len()), (8, 8));
        // Each inbound lane has straight plus one turn by default: 2 connections each.
        assert_eq!(s.links.len(), 16);
        assert!(s.ins.iter().all(|d| d.at.length() > 40.0));
    }

    #[test]
    fn a_drag_makes_one_exact_connection_and_a_click_cuts_it() {
        let (mut world, segs, node) = cross();
        let s = schem(&world, node);
        // The left lane of the east arm (segs[0]) and the lane nearest to the west arm's
        // outbound centre: connect it to the first outbound lane of the north arm (segs[1]).
        let i = s.ins.iter().position(|d| d.segment == segs[0]).unwrap();
        let o = s.outs.iter().position(|d| d.segment == segs[1]).unwrap();
        let c = connect_command(&s, node, i, o).expect("new connection");
        apply(&mut world, c);
        let s2 = schem(&world, node);
        assert!(s2.links.iter().any(|&(a, b)| {
            s2.ins[a].segment == segs[0]
                && s2.ins[a].index == s.ins[i].index
                && s2.outs[b].segment == segs[1]
                && s2.outs[b].index == s.outs[o].index
        }));
        // Exactly those lanes: that connection exists once.
        let n = s2
            .links
            .iter()
            .filter(|&&(a, b)| {
                s2.ins[a].segment == segs[0]
                    && s2.ins[a].index == s.ins[i].index
                    && s2.outs[b].segment == segs[1]
                    && s2.outs[b].index == s.outs[o].index
            })
            .count();
        assert_eq!(n, 1);
        // Joining it again is no new command.
        let i2 = s2
            .ins
            .iter()
            .position(|d| d.segment == segs[0] && d.index == s.ins[i].index)
            .unwrap();
        let o2 = s2
            .outs
            .iter()
            .position(|d| d.segment == segs[1] && d.index == s.outs[o].index)
            .unwrap();
        assert!(connect_command(&s2, node, i2, o2).is_none());
        // A click on the line cuts it.
        let k = s2
            .links
            .iter()
            .position(|&l| l == (i2, o2))
            .expect("the link");
        let mid = (s2.ins[i2].at + s2.outs[o2].at) * 0.5;
        assert_eq!(pick_link(&s2, mid, 2.0), Some(k));
        let c = disconnect_command(&s2, node, k).expect("cut");
        apply(&mut world, c);
        let s3 = schem(&world, node);
        assert!(!s3.links.iter().any(|&(a, b)| s3.ins[a].segment == segs[0]
            && s3.ins[a].index == s.ins[i].index
            && s3.outs[b].segment == segs[1]
            && s3.outs[b].index == s.outs[o].index));
    }

    #[test]
    fn the_last_connection_of_a_lane_is_not_cut() {
        let (world, _, node) = cross();
        let mut s = schem(&world, node);
        // Keep one link of the first inbound lane only.
        let keep = s.links.iter().position(|l| l.0 == 0).unwrap();
        let only = s.links[keep];
        s.links.retain(|l| l.0 != 0);
        s.links.push(only);
        let k = s.links.len() - 1;
        assert!(disconnect_command(&s, node, k).is_none());
    }
}

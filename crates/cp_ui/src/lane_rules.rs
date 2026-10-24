//! The Lanes mode of the Junctions tool (`architecture/system-design.md` §18.2, T-313): a mark
//! on every road arm of a junction; a click opens the arm's lanes, left to right as the driver
//! sees them, each with four switches (straight, right, left, U-turn). A switch sends
//! `SetLaneRule` with the arms of the chosen turns. "By default" takes the node's rules away.
//! The sim refuses a rule that strands an arm or a lane; the panel only leaves a lane its last
//! turn.

use bevy::math::{Vec2, Vec3Swizzles};
use bevy_egui::egui;
use cp_content::LaneDir;
use cp_locale::tr;
use cp_net::{
    Arm, Exit, Junction, JunctionGeometry, NetCommand, NetGraph, NodeClass, NodeId, SegmentId,
    Turn, turn_of_arms,
};
use cp_sim::{SimCommand, SimView};
use cp_tools::{ActiveTool, CommandOutbox, ToolState};

use crate::icons::Icon;
use crate::junction_marks::{MARK_RANGE_M, PICK_RADIUS, SIGN_BEYOND_CUT_M};

/// Side of an arm mark, points.
const MARK_SIDE: f32 = 26.0;
/// The marks stand this high above the ground, metres.
const LIFT_M: f32 = 1.0;
/// The turns of a switch row, in the order of its buttons.
pub const TURNS: [Turn; 4] = [Turn::Straight, Turn::Right, Turn::Left, Turn::UTurn];

/// One lane of an arm in the panel.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneRow {
    /// Index of the lane in its segment (the key of the rule).
    pub index: u8,
    /// The turns its connections make now, in [`TURNS`] order.
    pub on: [bool; 4],
    /// A rule of the player is set on it.
    pub ruled: bool,
}

/// The mark of one road arm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArmMark {
    pub node: NodeId,
    pub segment: SegmentId,
    pub at: Vec2,
}

/// The mode switch of the Junctions tool, in its row of the overlay panel (T-317f): lights and signs, or lane turns.
pub fn mode_buttons(ui: &mut egui::Ui, tools: &mut cp_tools::ToolState, size: f32, pad: f32) {
    for (lanes, icon, label) in [
        (false, Icon::Junctions, "Lights and signs"),
        (true, Icon::LanesMode, "Lanes"),
    ] {
        let r = icon
            .button(ui, size, pad, tools.lane_mode == lanes)
            .on_hover_text(tr(label));
        if r.clicked() {
            tools.lane_mode = lanes;
            tools.lane_arm = None;
        }
    }
}

fn arm_of(j: &Junction, segment: SegmentId) -> Option<&Arm> {
    j.arms.iter().find(|a| a.segment == segment)
}

/// The inbound car and bus lanes of the arm `segment` of `j`, left to right as the driver sees
/// them: `(index in the segment, lane id)`.
fn inbound(net: &NetGraph, a: &Arm) -> Vec<(u8, cp_net::LaneId)> {
    let Some(seg) = net.segment(a.segment) else {
        return Vec::new();
    };
    let mut v: Vec<(f32, u8, cp_net::LaneId)> = seg
        .lanes
        .iter()
        .filter_map(|&id| net.lane(id).map(|l| (id, l)))
        .filter(|(_, l)| l.kind.is_road_vehicle())
        .filter(|(_, l)| (l.dir == LaneDir::Forward) != a.at_start)
        .map(|(id, l)| {
            // Larger is more to the right of the travel direction.
            let right = if l.dir == LaneDir::Forward {
                -l.offset
            } else {
                l.offset
            };
            (right, l.index, id)
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|(_, i, id)| (i, id)).collect()
}

/// The marks of the junctions within `range` metres of `cam`, one per road arm with an inbound
/// car lane, in node-id order.
pub fn arm_marks(
    net: &NetGraph,
    junctions: &JunctionGeometry,
    cam: Vec2,
    range: f32,
) -> Vec<ArmMark> {
    let mut out = Vec::new();
    for (&node, j) in junctions.iter() {
        if j.class != NodeClass::Junction || j.pos.xz().distance(cam) > range {
            continue;
        }
        for a in &j.arms {
            if inbound(net, a).is_empty() {
                continue;
            }
            let dir = Vec2::new(a.angle.cos(), a.angle.sin());
            out.push(ArmMark {
                node,
                segment: a.segment,
                at: j.pos.xz() + dir * (a.setback + SIGN_BEYOND_CUT_M),
            });
        }
    }
    out
}

/// The lanes of the arm `segment` of `node`, left to right, with the turns they make.
pub fn lane_rows(
    net: &NetGraph,
    junctions: &JunctionGeometry,
    node: NodeId,
    segment: SegmentId,
) -> Vec<LaneRow> {
    let (Some(j), Some(n)) = (junctions.get(node), net.node(node)) else {
        return Vec::new();
    };
    let Some(a) = arm_of(j, segment) else {
        return Vec::new();
    };
    inbound(net, a)
        .into_iter()
        .map(|(index, id)| {
            let turns: Vec<Turn> = j
                .links
                .connections
                .iter()
                .filter(|c| c.from == id)
                .map(|c| c.turn)
                .collect();
            LaneRow {
                index,
                on: TURNS.map(|t| turns.contains(&t)),
                ruled: n
                    .lane_rules
                    .iter()
                    .any(|r| r.segment == segment && r.lane == index),
            }
        })
        .collect()
}

/// The turns the arm `segment` can make at all: some other road arm lies that way (a U-turn is
/// always possible), in [`TURNS`] order.
pub fn available_turns(net: &NetGraph, j: &Junction, segment: SegmentId) -> [bool; 4] {
    let Some(a) = arm_of(j, segment) else {
        return [false; 4];
    };
    let mut has = [false, false, false, true];
    for b in &j.arms {
        if b.segment == segment && b.at_start == a.at_start {
            continue;
        }
        let outbound = net.segment(b.segment).is_some_and(|s| {
            s.lanes
                .iter()
                .filter_map(|&l| net.lane(l))
                .any(|l| l.kind.is_road_vehicle() && (l.dir == LaneDir::Forward) == b.at_start)
        });
        if !outbound {
            continue;
        }
        if let Some(k) = TURNS.iter().position(|&t| t == turn_of_arms(a, b)) {
            has[k] = true;
        }
    }
    has
}

/// The command a click on switch `turn` (an index into [`TURNS`]) of lane `index` of the arm
/// sends: the lane's turns with that one flipped, as the arms of those turns. `None` when it
/// would leave the lane without a turn or the turn is not available.
pub fn toggle_command(
    net: &NetGraph,
    junctions: &JunctionGeometry,
    node: NodeId,
    segment: SegmentId,
    index: u8,
    turn: usize,
) -> Option<NetCommand> {
    let j = junctions.get(node)?;
    let a = arm_of(j, segment)?;
    let row = lane_rows(net, junctions, node, segment)
        .into_iter()
        .find(|r| r.index == index)?;
    if !available_turns(net, j, segment)[turn] {
        return None;
    }
    let mut on = row.on;
    on[turn] = !on[turn];
    if !on.iter().any(|&b| b) {
        return None;
    }
    let exits: Vec<Exit> = j
        .arms
        .iter()
        .filter(|b| {
            TURNS
                .iter()
                .position(|&t| t == turn_of_arms(a, b))
                .is_some_and(|k| on[k])
        })
        .map(|b| Exit {
            arm: b.segment,
            lane: None,
        })
        .collect();
    Some(NetCommand::SetLaneRule {
        node,
        segment,
        lane: index,
        exits: Some(exits),
    })
}

/// The commands that give the node its default lanes back: one per rule.
pub fn reset_commands(net: &NetGraph, node: NodeId) -> Vec<NetCommand> {
    net.node(node).map_or_else(Vec::new, |n| {
        n.lane_rules
            .iter()
            .map(|r| NetCommand::SetLaneRule {
                node,
                segment: r.segment,
                lane: r.lane,
                exits: None,
            })
            .collect()
    })
}

fn turn_icon(t: Turn) -> Icon {
    match t {
        Turn::Straight => Icon::LaneStraight,
        Turn::Right => Icon::LaneRight,
        Turn::Left => Icon::LaneLeft,
        Turn::UTurn => Icon::LaneUturn,
    }
}

fn turn_label(t: Turn) -> String {
    match t {
        Turn::Straight => tr("Straight"),
        Turn::Right => tr("Right"),
        Turn::Left => tr("Left"),
        Turn::UTurn => tr("U-turn"),
    }
}

/// The arm mark nearest to `cursor` (screen points) within [`PICK_RADIUS`].
pub fn pick(points: &[(Vec2, ArmMark)], cursor: Vec2) -> Option<ArmMark> {
    points
        .iter()
        .filter(|(p, _)| p.distance(cursor) <= PICK_RADIUS)
        .min_by(|a, b| a.0.distance(cursor).total_cmp(&b.0.distance(cursor)))
        .map(|(_, m)| *m)
}

/// One frame of the Lanes mode: the arm marks, the panel of the chosen arm, and the clicks.
pub fn lane_ui(
    ctx: &egui::Context,
    view: &SimView,
    tools: &mut ToolState,
    cam: Vec2,
    to_screen: impl Fn(Vec2) -> Option<Vec2>,
    outbox: &mut CommandOutbox,
    right_gap: f32,
) {
    let marks = arm_marks(&view.net, &view.junctions, cam, MARK_RANGE_M);
    let points: Vec<(Vec2, ArmMark)> = marks
        .iter()
        .filter_map(|m| to_screen(m.at).map(|p| (p, *m)))
        .collect();
    let pointer = ctx
        .pointer_latest_pos()
        .filter(|_| !crate::pointer_over_panels(ctx));
    let target = pointer.and_then(|p| pick(&points, Vec2::new(p.x, p.y)));
    egui::Area::new(egui::Id::new("lane_marks"))
        .order(egui::Order::Background)
        .interactable(false)
        .fixed_pos(egui::Pos2::ZERO)
        .show(ctx, |ui| {
            for (p, m) in &points {
                let rect = egui::Rect::from_center_size(
                    egui::pos2(p.x, p.y),
                    egui::vec2(MARK_SIDE, MARK_SIDE),
                );
                Icon::LanesMode.sized(MARK_SIDE).paint_at(ui, rect);
                let chosen = tools.lane_arm == Some((m.node, m.segment));
                if chosen || target == Some(*m) {
                    ui.painter().rect_stroke(
                        rect.expand(2.0),
                        4.0,
                        egui::Stroke::new(2.0, egui::Color32::WHITE),
                        egui::StrokeKind::Outside,
                    );
                }
            }
        });
    if let Some(m) = target.filter(|_| ctx.input(|i| i.pointer.primary_clicked())) {
        tools.lane_arm = Some((m.node, m.segment));
    }
    let Some((node, segment)) = tools.lane_arm else {
        return;
    };
    let rows = lane_rows(&view.net, &view.junctions, node, segment);
    let Some(j) = view.junctions.get(node) else {
        tools.lane_arm = None;
        return;
    };
    let can = available_turns(&view.net, j, segment);
    let mut open = true;
    let mut conn = tools.lane_connectors;
    egui::Window::new(tr("Lanes of this approach"))
        .id(egui::Id::new("lane_panel"))
        .anchor(
            egui::Align2::RIGHT_CENTER,
            egui::vec2(-(right_gap + 12.0), 0.0),
        )
        .resizable(false)
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.weak(tr("Left to right, as the driver sees them"));
            for (n, row) in rows.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("{}", n + 1));
                    for (k, &t) in TURNS.iter().enumerate() {
                        let r = ui
                            .add_enabled_ui(can[k], |ui| {
                                crate::panels::tool_button(
                                    ui,
                                    turn_icon(t),
                                    &turn_label(t),
                                    row.on[k],
                                )
                            })
                            .inner;
                        if r.clicked()
                            && let Some(c) = toggle_command(
                                &view.net,
                                &view.junctions,
                                node,
                                segment,
                                row.index,
                                k,
                            )
                        {
                            outbox.push(SimCommand::Net(c));
                        }
                    }
                });
            }
            ui.add_space(4.0);
            let any = !reset_commands(&view.net, node).is_empty();
            if ui
                .add_enabled(
                    any,
                    egui::Button::image_and_text(Icon::LaneReset.sized(18.0), tr("By default")),
                )
                .clicked()
            {
                for c in reset_commands(&view.net, node) {
                    outbox.push(SimCommand::Net(c));
                }
            }
            ui.toggle_value(&mut conn, tr("Connectors"));
        });
    if conn && open {
        crate::connectors::connectors_window(ctx, view, tools, outbox, &mut conn);
    }
    tools.lane_connectors = conn;
    if !open {
        tools.lane_arm = None;
    }
}

/// Draws the Lanes mode (T-313) while the Junctions tool is in hand with it switched on.
pub fn draw_lane_editor(
    mut contexts: bevy_egui::EguiContexts,
    view: Option<bevy::prelude::Res<SimView>>,
    mut tools: bevy::prelude::ResMut<ToolState>,
    mut outbox: bevy::prelude::ResMut<CommandOutbox>,
    state: bevy::prelude::Res<crate::panels::UiState>,
    cameras: bevy::prelude::Query<(&bevy::prelude::Camera, &bevy::prelude::GlobalTransform)>,
) {
    let Some(view) = view else { return };
    if tools.active != ActiveTool::Junctions || !tools.lane_mode {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let Ok((camera, transform)) = cameras.single() else {
        return;
    };
    let cam = transform.translation();
    let terrain = view.terrain.clone();
    let to_screen = |at: Vec2| {
        let y = terrain.as_deref().map_or(0.0, |t| t.height(at)) + LIFT_M;
        camera
            .world_to_viewport(transform, bevy::math::Vec3::new(at.x, y, at.y))
            .ok()
    };
    lane_ui(
        ctx,
        &view,
        &mut tools,
        Vec2::new(cam.x, cam.z),
        to_screen,
        &mut outbox,
        // The overlay panel (with the mode switch) stays visible beside the window.
        crate::overlays_panel::width(&state),
    );
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

    fn star(arms: &[(f32, &str)]) -> (World, Vec<SegmentId>) {
        let db = Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        );
        let mut g = NetGraph::new();
        let mut segs = Vec::new();
        for &(deg, name) in arms {
            let road = db.road_id(name).unwrap();
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
        let mut world = World::new();
        world.insert_resource(g);
        world.init_resource::<JunctionGeometry>();
        world.insert_resource(SimContent(db));
        world.run_system_cached(update_junctions).unwrap();
        (world, segs)
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

    fn rows(world: &World, node: NodeId, seg: SegmentId) -> Vec<LaneRow> {
        lane_rows(
            world.resource::<NetGraph>(),
            world.resource::<JunctionGeometry>(),
            node,
            seg,
        )
    }

    #[test]
    fn an_arm_lists_its_inbound_lanes_left_to_right_with_the_default_turns() {
        let (world, segs) = star(&[
            (0.0, "medium_4l"),
            (90.0, "medium_4l"),
            (180.0, "medium_4l"),
            (270.0, "medium_4l"),
        ]);
        let node = world.resource::<NetGraph>().find_node(C).unwrap();
        let r = rows(&world, node, segs[0]);
        assert_eq!(r.len(), 2);
        // Left lane: straight and left; right lane: straight and right (§6.4).
        assert_eq!(r[0].on, [true, false, true, false]);
        assert_eq!(r[1].on, [true, true, false, false]);
        assert!(r.iter().all(|x| !x.ruled));
        let marks = arm_marks(
            world.resource::<NetGraph>(),
            world.resource::<JunctionGeometry>(),
            Vec2::new(C.x, C.z),
            MARK_RANGE_M,
        );
        assert_eq!(marks.len(), 4, "one mark per road arm");
    }

    #[test]
    fn a_switch_sends_a_rule_the_sim_accepts_and_the_rows_follow() {
        let (mut world, segs) = star(&[
            (0.0, "large_6l"),
            (90.0, "large_6l"),
            (180.0, "large_6l"),
            (270.0, "large_6l"),
        ]);
        let node = world.resource::<NetGraph>().find_node(C).unwrap();
        let r = rows(&world, node, segs[0]);
        assert_eq!(r.len(), 3);
        // The middle lane gets the right turn (index 1 in TURNS), the right lane then drops it.
        let (mid, right) = (r[1].index, r[2].index);
        let c = toggle_command(
            world.resource::<NetGraph>(),
            world.resource::<JunctionGeometry>(),
            node,
            segs[0],
            mid,
            1,
        )
        .expect("a command");
        apply(&mut world, c);
        assert_eq!(
            rows(&world, node, segs[0])[1].on,
            [true, true, false, false]
        );
        assert!(rows(&world, node, segs[0])[1].ruled);
        let c = toggle_command(
            world.resource::<NetGraph>(),
            world.resource::<JunctionGeometry>(),
            node,
            segs[0],
            right,
            1,
        )
        .expect("a command");
        apply(&mut world, c);
        assert_eq!(
            rows(&world, node, segs[0])[2].on,
            [true, false, false, false]
        );
        // The last turn of a lane cannot be switched off.
        assert!(
            toggle_command(
                world.resource::<NetGraph>(),
                world.resource::<JunctionGeometry>(),
                node,
                segs[0],
                right,
                0,
            )
            .is_none()
        );
        // A U-turn can be added from the left lane.
        let left = rows(&world, node, segs[0])[0].index;
        let c = toggle_command(
            world.resource::<NetGraph>(),
            world.resource::<JunctionGeometry>(),
            node,
            segs[0],
            left,
            3,
        )
        .unwrap();
        apply(&mut world, c);
        assert_eq!(rows(&world, node, segs[0])[0].on, [true, false, true, true]);
        // "By default" takes every rule away.
        let resets = reset_commands(world.resource::<NetGraph>(), node);
        assert_eq!(resets.len(), 3);
        for c in resets {
            apply(&mut world, c);
        }
        assert_eq!(
            rows(&world, node, segs[0])[0].on,
            [true, false, true, false]
        );
        assert!(rows(&world, node, segs[0]).iter().all(|x| !x.ruled));
    }

    #[test]
    fn a_turn_nobody_lies_that_way_is_not_offered() {
        // A T: the stem (270°) has no straight exit.
        let (world, segs) = star(&[
            (0.0, "medium_4l"),
            (180.0, "medium_4l"),
            (270.0, "medium_4l"),
        ]);
        let node = world.resource::<NetGraph>().find_node(C).unwrap();
        let j = world.resource::<JunctionGeometry>().get(node).unwrap();
        let can = available_turns(world.resource::<NetGraph>(), j, segs[2]);
        assert_eq!(can, [false, true, true, true]);
        assert!(
            toggle_command(
                world.resource::<NetGraph>(),
                world.resource::<JunctionGeometry>(),
                node,
                segs[2],
                rows(&world, node, segs[2])[0].index,
                0,
            )
            .is_none()
        );
    }
}

//! The traffic-light and sign marks over the junctions (`architecture/system-design.md` §6.5a,
//! T-303): while the "Traffic lights" overlay is on (the Junctions tool switches it on), every
//! junction the player can control carries a light icon at its centre and, with the light off,
//! a sign icon on each road arm. A click with the Junctions tool on a light toggles it; on a
//! sign it steps the sign none → main → yield → none.

use bevy::math::{Vec2, Vec3Swizzles};
use bevy_egui::egui;
use cp_locale::tr;
use cp_net::{JunctionGeometry, NetCommand, NetGraph, NodeId, SegmentId, Sign};
use cp_render::{Overlay, OverlayState};
use cp_sim::{SimCommand, SimView};
use cp_tools::{ActiveTool, CommandOutbox, ToolState};

use crate::icons::Icon;

/// Marks are left out beyond this distance from the camera, metres.
pub const MARK_RANGE_M: f32 = 1500.0;
/// A sign stands this far beyond the arm's cut, metres.
pub const SIGN_BEYOND_CUT_M: f32 = 6.0;
/// A click or hover picks the nearest mark within this many points.
pub const PICK_RADIUS: f32 = 18.0;
/// Side of the light icon, points.
pub const LIGHT_SIDE: f32 = 32.0;
/// Side of a sign icon, points.
pub const SIGN_SIDE: f32 = 26.0;
/// The marks stand this high above the ground, metres.
pub const LIFT_M: f32 = 1.0;
/// The cursor is over a junction up to this far beyond its farthest cut, metres.
const HOVER_MARGIN_M: f32 = 2.0;
/// How far from the pointer the label stands, points (as the road tool's).
const LABEL_OFFSET: egui::Vec2 = egui::vec2(18.0, 20.0);
const MAIN_FRAME: egui::Color32 = egui::Color32::from_rgb(242, 193, 78);

/// The sign mark of one road arm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArmMark {
    /// Where the icon stands (world XZ).
    pub at: Vec2,
    pub segment: SegmentId,
    pub sign: Option<Sign>,
    /// No sign, but the arm is on the main road all the same (rank): a thin yellow frame.
    pub auto_main: bool,
}

/// The marks of one controllable junction.
#[derive(Clone, Debug, PartialEq)]
pub struct JunctionMark {
    pub node: NodeId,
    /// The node (world XZ).
    pub at: Vec2,
    pub light: bool,
    /// The sign marks; none while the light is on (the signs do nothing then).
    pub arms: Vec<ArmMark>,
}

/// What a click would act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkTarget {
    Light(NodeId),
    Sign(NodeId, SegmentId),
}

fn carries_cars(net: &NetGraph, seg: SegmentId) -> bool {
    net.segment(seg).is_some_and(|s| {
        s.lanes
            .iter()
            .any(|&l| net.lane(l).is_some_and(|l| l.kind.is_road_vehicle()))
    })
}

/// The marks of the controllable junctions within `range` metres of `cam`, in node-id order.
pub fn junction_marks(
    net: &NetGraph,
    junctions: &JunctionGeometry,
    cam: Vec2,
    range: f32,
) -> Vec<JunctionMark> {
    junctions
        .iter()
        .filter(|(id, j)| j.pos.xz().distance(cam) <= range && net.is_controllable(**id))
        .filter_map(|(&id, j)| {
            let node = net.node(id)?;
            let light = node.control.has_light();
            let arms = if light {
                Vec::new()
            } else {
                j.arms
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| carries_cars(net, a.segment))
                    .map(|(i, a)| {
                        let sign = node
                            .signs
                            .iter()
                            .find(|s| s.segment == a.segment)
                            .map(|s| s.sign);
                        let dir = Vec2::new(a.angle.cos(), a.angle.sin());
                        ArmMark {
                            at: j.pos.xz() + dir * (a.setback + SIGN_BEYOND_CUT_M),
                            segment: a.segment,
                            sign,
                            auto_main: sign.is_none() && j.control.main_arms.contains(&i),
                        }
                    })
                    .collect()
            };
            Some(JunctionMark {
                node: id,
                at: j.pos.xz(),
                light,
                arms,
            })
        })
        .collect()
}

/// The sign a click on the arm `segment` of `node` puts there: none → main → yield → none,
/// skipping main when the node has two main arms elsewhere already. `None` if nothing.
pub fn next_sign(net: &NetGraph, node: NodeId, segment: SegmentId) -> Option<Sign> {
    let signs = net.node(node).map_or(&[][..], |n| &n.signs[..]);
    let now = signs.iter().find(|s| s.segment == segment).map(|s| s.sign);
    let mains = signs
        .iter()
        .filter(|s| s.segment != segment && s.sign == Sign::Main)
        .count();
    match now {
        None if mains < 2 => Some(Sign::Main),
        None | Some(Sign::Main) => Some(Sign::Yield),
        Some(Sign::Yield) => None,
    }
}

/// Whether a click on an empty sign skips "main" (two main arms already): the cursor says why.
pub fn main_is_full(net: &NetGraph, node: NodeId, segment: SegmentId) -> bool {
    let signs = net.node(node).map_or(&[][..], |n| &n.signs[..]);
    signs.iter().all(|s| s.segment != segment)
        && signs.iter().filter(|s| s.sign == Sign::Main).count() >= 2
}

/// The command a click on `target` sends.
pub fn click_command(net: &NetGraph, target: MarkTarget) -> NetCommand {
    match target {
        MarkTarget::Light(node) => NetCommand::ToggleTrafficLight { node },
        MarkTarget::Sign(node, segment) => NetCommand::SetSign {
            node,
            segment,
            sign: next_sign(net, node, segment),
        },
    }
}

/// The nearest of `points` (screen positions of the marks) within [`PICK_RADIUS`] of `cursor`.
/// Ties go to the earlier point.
pub fn pick(points: &[(Vec2, MarkTarget)], cursor: Vec2) -> Option<MarkTarget> {
    points
        .iter()
        .filter(|(p, _)| p.distance(cursor) <= PICK_RADIUS)
        .min_by(|a, b| a.0.distance(cursor).total_cmp(&b.0.distance(cursor)))
        .map(|(_, t)| *t)
}

/// Whether the marks show: the overlay is on, or the Junctions tool is in hand.
pub fn shown(overlay: &OverlayState, tools: &ToolState) -> bool {
    overlay.active == Overlay::Junctions || tools.active == ActiveTool::Junctions
}

/// The screen points of the marks (the light, then the signs, per junction); a mark
/// `to_screen` cannot place is left out.
pub fn mark_points(
    marks: &[JunctionMark],
    to_screen: impl Fn(Vec2) -> Option<Vec2>,
) -> Vec<(Vec2, MarkTarget)> {
    let mut out = Vec::new();
    for m in marks {
        out.extend(to_screen(m.at).map(|p| (p, MarkTarget::Light(m.node))));
        for a in &m.arms {
            out.extend(to_screen(a.at).map(|p| (p, MarkTarget::Sign(m.node, a.segment))));
        }
    }
    out
}

/// What a click on `target` would do, for the label beside the cursor.
pub fn action_text(net: &NetGraph, target: MarkTarget) -> String {
    match target {
        MarkTarget::Light(node) => {
            if net.node(node).is_some_and(|n| n.control.has_light()) {
                tr("Click: remove the traffic light")
            } else {
                tr("Click: put up a traffic light")
            }
        }
        MarkTarget::Sign(node, segment) if main_is_full(net, node, segment) => {
            tr("Two main arms at most: click for yield")
        }
        MarkTarget::Sign(node, segment) => match next_sign(net, node, segment) {
            Some(Sign::Main) => tr("Click: main road"),
            Some(Sign::Yield) => tr("Click: give way"),
            None => tr("Click: no sign"),
        },
    }
}

/// The junction at `at` (world XZ) that the player cannot control (only two-lane roads meet
/// there), if any.
pub fn automatic_junction_at(
    net: &NetGraph,
    junctions: &JunctionGeometry,
    at: Vec2,
) -> Option<NodeId> {
    junctions
        .iter()
        .find(|(id, j)| {
            let reach = j.arms.iter().map(|a| a.setback).fold(0.0, f32::max) + HOVER_MARGIN_M;
            j.arms.len() >= 3 && j.pos.xz().distance(at) <= reach && !net.is_controllable(**id)
        })
        .map(|(id, _)| *id)
}

/// One frame of the marks while they show: draws them, labels what the cursor is over and,
/// with the Junctions tool in hand, turns a click on a mark into its command. `to_screen`
/// projects a point of the ground (world XZ) to the screen; `cam` is the camera (world XZ).
pub fn junction_ui(
    ctx: &egui::Context,
    view: &SimView,
    tools: &ToolState,
    cam: Vec2,
    to_screen: impl Fn(Vec2) -> Option<Vec2>,
    outbox: &mut CommandOutbox,
) {
    let marks = junction_marks(&view.net, &view.junctions, cam, MARK_RANGE_M);
    let points = mark_points(&marks, &to_screen);
    let pointer = ctx
        .pointer_latest_pos()
        .filter(|_| !crate::pointer_over_panels(ctx));
    let target = pointer.and_then(|p| pick(&points, Vec2::new(p.x, p.y)));
    if !marks.is_empty() {
        egui::Area::new(egui::Id::new("junction_marks"))
            .order(egui::Order::Background)
            .interactable(false)
            .fixed_pos(egui::Pos2::ZERO)
            .show(ctx, |ui| {
                for m in &marks {
                    for a in &m.arms {
                        let Some(p) = to_screen(a.at) else { continue };
                        let icon = match a.sign {
                            Some(Sign::Main) => Icon::SignMain,
                            Some(Sign::Yield) => Icon::SignYield,
                            None => Icon::SignNone,
                        };
                        let rect = square(p, SIGN_SIDE);
                        icon.sized(SIGN_SIDE).paint_at(ui, rect);
                        if a.auto_main {
                            frame(ui, rect, MAIN_FRAME, 1.5);
                        }
                        if target == Some(MarkTarget::Sign(m.node, a.segment)) {
                            frame(ui, rect.expand(2.0), egui::Color32::WHITE, 2.0);
                        }
                    }
                    let Some(p) = to_screen(m.at) else { continue };
                    let icon = if m.light {
                        Icon::MarkLightOn
                    } else {
                        Icon::MarkLightOff
                    };
                    let rect = square(p, LIGHT_SIDE);
                    icon.sized(LIGHT_SIDE).paint_at(ui, rect);
                    if target == Some(MarkTarget::Light(m.node)) {
                        frame(ui, rect.expand(2.0), egui::Color32::WHITE, 2.0);
                    }
                }
            });
    }
    if tools.active != ActiveTool::Junctions {
        return;
    }
    let Some(pos) = pointer else { return };
    let text = match target {
        Some(t) => Some((action_text(&view.net, t), true)),
        None => tools
            .hover
            .and_then(|h| automatic_junction_at(&view.net, &view.junctions, h))
            .map(|_| (tr("Two-lane roads only: automatic"), false)),
    };
    if let Some((text, ok)) = text {
        egui::Area::new(egui::Id::new("junction_cursor_label"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + LABEL_OFFSET)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let color = if ok {
                        egui::Color32::LIGHT_GREEN
                    } else {
                        egui::Color32::LIGHT_GRAY
                    };
                    ui.colored_label(color, text);
                });
            });
    }
    if let Some(t) = target.filter(|_| ctx.input(|i| i.pointer.primary_clicked())) {
        outbox.push(SimCommand::Net(click_command(&view.net, t)));
    }
}

fn square(center: Vec2, side: f32) -> egui::Rect {
    egui::Rect::from_center_size(egui::pos2(center.x, center.y), egui::vec2(side, side))
}

fn frame(ui: &egui::Ui, rect: egui::Rect, color: egui::Color32, width: f32) {
    ui.painter().rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(width, color),
        egui::StrokeKind::Outside,
    );
}

/// Draws the junction marks (T-303) while the overlay is on or the Junctions tool is in hand.
pub fn draw_junction_marks(
    mut contexts: bevy_egui::EguiContexts,
    view: Option<bevy::prelude::Res<SimView>>,
    overlay: bevy::prelude::Res<OverlayState>,
    tools: bevy::prelude::Res<ToolState>,
    mut outbox: bevy::prelude::ResMut<CommandOutbox>,
    cameras: bevy::prelude::Query<(&bevy::prelude::Camera, &bevy::prelude::GlobalTransform)>,
) {
    let Some(view) = view else { return };
    if !shown(&overlay, &tools) || (tools.active == ActiveTool::Junctions && tools.lane_mode) {
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
    junction_ui(
        ctx,
        &view,
        &tools,
        Vec2::new(cam.x, cam.z),
        to_screen,
        &mut outbox,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;
    use bevy::math::Vec3;
    use cp_content::{ContentDb, SimContent};
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, JunctionControl, update_junctions};
    use std::sync::Arc;

    const C: Vec3 = Vec3::new(1000.0, 0.0, 1000.0);

    /// A junction of `arms` (angle in degrees, road) at `C`, with its geometry and plan.
    fn star(arms: &[(f32, &str)]) -> World {
        let db = Arc::new(
            ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
                .unwrap(),
        );
        let mut g = NetGraph::new();
        for &(deg, name) in arms {
            let road = db.road_id(name).unwrap();
            let a = deg.to_radians();
            let tip = C + Vec3::new(150.0 * a.cos(), 0.0, 150.0 * a.sin());
            g.add_segment(
                db.road(road),
                road,
                CubicBezier3::line(C, tip),
                Elevation::Ground,
            )
            .unwrap();
        }
        let mut world = World::new();
        world.insert_resource(g);
        world.init_resource::<JunctionGeometry>();
        world.insert_resource(SimContent(db));
        world.run_system_cached(update_junctions).unwrap();
        world
    }

    fn marks(world: &World) -> Vec<JunctionMark> {
        junction_marks(
            world.resource::<NetGraph>(),
            world.resource::<JunctionGeometry>(),
            Vec2::new(C.x, C.z),
            MARK_RANGE_M,
        )
    }

    fn centre(world: &World) -> NodeId {
        world.resource::<NetGraph>().find_node(C).unwrap()
    }

    #[test]
    fn a_controllable_junction_has_a_light_mark_and_a_sign_mark_per_road_arm() {
        let mut world = star(&[
            (0.0, "large_6l"),
            (90.0, "basic_2l"),
            (180.0, "basic_2l"),
            (270.0, "park_path"),
        ]);
        let m = marks(&world);
        assert_eq!(m.len(), 1, "only the junction, not the dead ends");
        assert!(!m[0].light);
        assert_eq!(m[0].arms.len(), 3, "the path gets no sign");
        // The avenue and the lower-id street are the main road by rank: framed, no sign.
        let framed: Vec<bool> = m[0].arms.iter().map(|a| a.auto_main).collect();
        assert_eq!(framed, vec![true, true, false]);
        // Each sign stands beyond the cut of its arm, on the arm's side.
        let a0 = m[0].arms[0];
        assert!(a0.at.x > C.x + SIGN_BEYOND_CUT_M && (a0.at.y - C.z).abs() < 1e-3);

        let n = centre(&world);
        world
            .resource_mut::<NetGraph>()
            .set_sign(n, m[0].arms[2].segment, Some(Sign::Main))
            .unwrap();
        world.run_system_cached(update_junctions).unwrap();
        let m = marks(&world);
        assert_eq!(m[0].arms[2].sign, Some(Sign::Main));
        assert!(!m[0].arms[2].auto_main);

        // With the light on only the light is shown.
        world
            .resource_mut::<NetGraph>()
            .toggle_traffic_light(n)
            .unwrap();
        world.run_system_cached(update_junctions).unwrap();
        let m = marks(&world);
        assert!(m[0].light && m[0].arms.is_empty());
        assert_eq!(
            world.resource::<NetGraph>().node(n).unwrap().control,
            JunctionControl::ManualTrafficLight
        );
    }

    #[test]
    fn two_lane_streets_and_far_junctions_have_no_marks() {
        let world = star(&[(0.0, "basic_2l"), (90.0, "basic_2l"), (180.0, "basic_2l")]);
        assert!(marks(&world).is_empty());
        let world = star(&[(0.0, "large_6l"), (90.0, "basic_2l"), (180.0, "basic_2l")]);
        let far = junction_marks(
            world.resource::<NetGraph>(),
            world.resource::<JunctionGeometry>(),
            Vec2::new(C.x + MARK_RANGE_M + 10.0, C.z),
            MARK_RANGE_M,
        );
        assert!(far.is_empty());
    }

    #[test]
    fn a_click_steps_the_sign_and_skips_a_third_main() {
        let mut world = star(&[
            (0.0, "large_6l"),
            (90.0, "basic_2l"),
            (180.0, "basic_2l"),
            (270.0, "basic_2l"),
        ]);
        let n = centre(&world);
        let segs = world
            .resource::<NetGraph>()
            .node(n)
            .unwrap()
            .segments
            .clone();
        let click = |world: &mut World, seg| {
            let cmd = click_command(world.resource::<NetGraph>(), MarkTarget::Sign(n, seg));
            let NetCommand::SetSign { sign, .. } = cmd else {
                panic!("{cmd:?}")
            };
            world
                .resource_mut::<NetGraph>()
                .set_sign(n, seg, sign)
                .unwrap();
            sign
        };
        assert_eq!(click(&mut world, segs[0]), Some(Sign::Main));
        assert_eq!(click(&mut world, segs[0]), Some(Sign::Yield));
        assert_eq!(click(&mut world, segs[0]), None);
        click(&mut world, segs[1]);
        click(&mut world, segs[2]);
        assert!(main_is_full(world.resource::<NetGraph>(), n, segs[3]));
        assert_eq!(
            click(&mut world, segs[3]),
            Some(Sign::Yield),
            "main skipped"
        );
        assert_eq!(
            click_command(world.resource::<NetGraph>(), MarkTarget::Light(n)),
            NetCommand::ToggleTrafficLight { node: n }
        );
    }

    #[test]
    fn the_nearest_mark_within_the_radius_is_picked() {
        let n: NodeId = cp_core::Id::new(1, 0);
        let s: SegmentId = cp_core::Id::new(2, 0);
        let pts = [
            (Vec2::new(100.0, 100.0), MarkTarget::Light(n)),
            (Vec2::new(110.0, 100.0), MarkTarget::Sign(n, s)),
        ];
        assert_eq!(
            pick(&pts, Vec2::new(107.0, 100.0)),
            Some(MarkTarget::Sign(n, s))
        );
        assert_eq!(
            pick(&pts, Vec2::new(101.0, 99.0)),
            Some(MarkTarget::Light(n))
        );
        assert_eq!(
            pick(&pts, Vec2::new(100.0, 100.0 + PICK_RADIUS + 1.0)),
            None
        );
    }

    /// The rig: one frame of [`junction_ui`] over `view`, the ground drawn 1:1 with `C` at the
    /// middle of the 1280×720 screen.
    fn rig_frame(
        ctx: &egui::Context,
        t: f64,
        events: Vec<egui::Event>,
        view: &SimView,
        tools: &ToolState,
        outbox: &mut CommandOutbox,
    ) -> Vec<String> {
        let mid = Vec2::new(640.0, 360.0);
        crate::testutil::frame_texts(ctx, crate::testutil::input(t, events), |ui| {
            junction_ui(
                ui.ctx(),
                view,
                tools,
                Vec2::new(C.x, C.z),
                |at| Some(at - Vec2::new(C.x, C.z) + mid),
                outbox,
            );
        })
        .into_iter()
        .map(|(s, _)| s)
        .collect()
    }

    fn view_of(world: &World) -> SimView {
        let mut view = cp_sim::SimRunner::new(1).view();
        view.net = Arc::new(world.resource::<NetGraph>().clone());
        view.junctions = world.resource::<JunctionGeometry>().clone();
        view
    }

    fn click_at(
        ctx: &egui::Context,
        t: &mut f64,
        at: egui::Pos2,
        view: &SimView,
        tools: &ToolState,
    ) -> Vec<SimCommand> {
        let mut outbox = CommandOutbox::default();
        for events in crate::testutil::click(at) {
            *t += 0.1;
            rig_frame(ctx, *t, events, view, tools, &mut outbox);
        }
        outbox.iter().cloned().collect()
    }

    #[test]
    fn a_click_on_a_mark_with_the_tool_sends_its_command_and_the_cursor_says_what_it_does() {
        let world = star(&[(0.0, "large_6l"), (90.0, "basic_2l"), (180.0, "basic_2l")]);
        let view = view_of(&world);
        let n = centre(&world);
        let m = marks(&world);
        let ctx = egui::Context::default();
        let mut t = 0.0;
        let mut tools = ToolState::default();
        tools.select(ActiveTool::Junctions);

        let light = click_at(&ctx, &mut t, egui::pos2(640.0, 360.0), &view, &tools);
        assert_eq!(
            light,
            vec![SimCommand::Net(NetCommand::ToggleTrafficLight { node: n })]
        );

        let a = m[0].arms[2];
        let sign_at = egui::pos2(640.0 + a.at.x - C.x, 360.0 + a.at.y - C.z);
        let sign = click_at(&ctx, &mut t, sign_at, &view, &tools);
        assert_eq!(
            sign,
            vec![SimCommand::Net(NetCommand::SetSign {
                node: n,
                segment: a.segment,
                sign: Some(Sign::Main),
            })]
        );
        // Hovering the sign labels the click.
        t += 0.1;
        let texts = rig_frame(
            &ctx,
            t,
            vec![egui::Event::PointerMoved(sign_at)],
            &view,
            &tools,
            &mut CommandOutbox::default(),
        );
        assert!(texts.iter().any(|s| s == "Click: main road"), "{texts:?}");

        // With only the overlay on, the marks show but a click does nothing.
        tools.select(ActiveTool::None);
        let none = click_at(&ctx, &mut t, egui::pos2(640.0, 360.0), &view, &tools);
        assert!(none.is_empty());
    }

    #[test]
    fn over_a_junction_of_two_lane_roads_the_cursor_says_it_is_automatic() {
        let world = star(&[(0.0, "basic_2l"), (90.0, "basic_2l"), (180.0, "basic_2l")]);
        let view = view_of(&world);
        let mut tools = ToolState::default();
        tools.select(ActiveTool::Junctions);
        tools.hover = Some(Vec2::new(C.x + 1.0, C.z));
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        // A new label area shows from its second frame (egui sizes it on the first).
        for t in [0.1, 0.2] {
            texts = rig_frame(
                &ctx,
                t,
                vec![egui::Event::PointerMoved(egui::pos2(641.0, 360.0))],
                &view,
                &tools,
                &mut CommandOutbox::default(),
            );
        }
        assert!(
            texts.iter().any(|s| s == "Two-lane roads only: automatic"),
            "{texts:?}"
        );
    }
}

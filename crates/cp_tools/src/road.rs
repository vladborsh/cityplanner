//! The road tool: modes, click state machine, preview.

use bevy_math::{Vec2, Vec3};
use cp_content::{RoadDef, RoadDefId};
use cp_core::Cost;
use cp_core::curve::{CubicBezier3, to_xz};
use cp_net::{
    Anchor, BuildPlan, ElevationMode, PlanPiece, RoadBuildError, RoadBuildPlan, RoadPath,
    RoadPathRequest, SnapOptions, SnapReport, SnapReportKind, clearance_report,
    validate_build_plan,
};
use cp_sim::SimView;

use crate::snap::{self, Hit, SnapCtx, SnapEdge, SnapNode, resolve};

/// Metres of road height per PgUp/PgDn step.
pub const ELEVATION_STEP: f32 = 5.0;
/// Highest elevation level (`level * ELEVATION_STEP` metres).
pub const MAX_ELEVATION_LEVEL: u8 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoadMode {
    #[default]
    Straight,
    /// Three clicks: start, control point, end.
    Curve,
    /// A polyline; Enter (or a click on the last point) finishes it.
    Free,
}

/// Depth of the axis of a road tunnel the tool draws, metres (the content's default depth).
pub const ROAD_TUNNEL_DEPTH: f32 = 12.0;

/// State of the road tool.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoadTool {
    pub mode: RoadMode,
    pub road: Option<RoadDefId>,
    /// 0 = ground, otherwise raised by `level * ELEVATION_STEP`.
    pub level: u8,
    /// Depth of the axis below the terrain for a road that is built in a tunnel only (the
    /// metro track, T-164): the tool then draws `Tunnel(depth)` and the height keys do nothing.
    pub tunnel_depth: Option<f32>,
    /// A road type that has a tunnel is built under the ground (T-173): the height keys go
    /// below ground level (PgDn from level 0) and the tool draws `Tunnel(ROAD_TUNNEL_DEPTH)`.
    pub underground: bool,
    /// Points clicked so far in the current drawing.
    pub points: Vec<Vec2>,
}

impl RoadTool {
    pub fn elevation(&self) -> ElevationMode {
        if let Some(depth) = self.tunnel_depth {
            ElevationMode::Tunnel(depth)
        } else if self.underground {
            ElevationMode::Tunnel(ROAD_TUNNEL_DEPTH)
        } else if self.level == 0 {
            ElevationMode::Ground
        } else {
            ElevationMode::Elevated(f32::from(self.level) * ELEVATION_STEP)
        }
    }

    pub fn raise(&mut self) {
        if self.tunnel_depth.is_some() {
            return;
        }
        if self.underground {
            self.underground = false;
            return;
        }
        self.level = (self.level + 1).min(MAX_ELEVATION_LEVEL);
    }

    pub fn lower(&mut self) {
        if self.tunnel_depth.is_some() {
            return;
        }
        if self.level == 0 {
            self.underground = true;
            return;
        }
        self.level = self.level.saturating_sub(1);
    }

    pub fn set_mode(&mut self, mode: RoadMode) {
        if self.mode != mode {
            self.mode = mode;
            self.points.clear();
        }
    }

    pub fn cancel(&mut self) {
        self.points.clear();
    }

    /// Clicks needed before the path is complete (`None` for free mode, which ends on demand).
    fn clicks_needed(&self) -> Option<usize> {
        match self.mode {
            RoadMode::Straight => Some(2),
            RoadMode::Curve => Some(3),
            RoadMode::Free => None,
        }
    }

    /// The path as it would be if the cursor were at `hover` (what the ghost shows). With
    /// nothing clicked yet there is nothing to preview.
    pub fn path_with_hover(&self, hover: Vec2) -> Option<RoadPath> {
        let first = *self.points.first()?;
        Some(match (self.mode, self.points.len()) {
            (RoadMode::Straight, _) | (RoadMode::Curve, 1) => RoadPath::Straight {
                from: first,
                to: hover,
            },
            (RoadMode::Curve, _) => RoadPath::Curve {
                from: first,
                control: self.points[1],
                to: hover,
            },
            (RoadMode::Free, _) => {
                let mut pts = self.points.clone();
                pts.push(hover);
                RoadPath::Free(pts)
            }
        })
    }

    /// The request for `path` with the tool's road type, elevation and the default snaps (the
    /// ones the `BuildRoad` command uses, so the preview and the command agree).
    pub fn request(&self, path: RoadPath) -> Option<RoadPathRequest> {
        Some(RoadPathRequest {
            road: self.road?,
            path,
            elevation: self.elevation(),
            snap: SnapOptions::default(),
        })
    }

    /// A click at `p`. Returns the finished request when this click completed the drawing; the
    /// tool then stays in a chain (T-176): the next piece starts at the last point (a curve's
    /// tangent is free). `Esc` ([`Self::cancel`]) ends the chain.
    pub fn click(&mut self, p: Vec2) -> Option<RoadPathRequest> {
        self.road?;
        if self.mode == RoadMode::Free && self.points.last().is_some_and(|l| l.distance(p) < 1.0) {
            return self.finish_free();
        }
        self.points.push(p);
        if self.clicks_needed() == Some(self.points.len()) {
            let pts = std::mem::replace(&mut self.points, vec![p]);
            let path = match self.mode {
                RoadMode::Straight => RoadPath::Straight {
                    from: pts[0],
                    to: pts[1],
                },
                RoadMode::Curve => RoadPath::Curve {
                    from: pts[0],
                    control: pts[1],
                    to: pts[2],
                },
                RoadMode::Free => unreachable!("free mode has no fixed length"),
            };
            return self.request(path);
        }
        None
    }

    /// Ends a free-mode drawing (needs at least two points).
    pub fn finish_free(&mut self) -> Option<RoadPathRequest> {
        if self.mode != RoadMode::Free || self.points.len() < 2 {
            return None;
        }
        let last = *self.points.last()?;
        let pts = std::mem::replace(&mut self.points, vec![last]);
        self.request(RoadPath::Free(pts))
    }
}

/// The end point of a path.
fn path_end(path: &RoadPath) -> Option<Vec2> {
    match path {
        RoadPath::Straight { to, .. } | RoadPath::Curve { to, .. } => Some(*to),
        RoadPath::Free(pts) => pts.last().copied(),
    }
}

/// Settles a click that completed a piece (T-176): `tool` already stands in the chain state
/// ([`RoadTool::click`]); `before` is the tool as it was before the click. The points of the
/// piece are the ones the ghost showed (a click takes the ghost's resolved point, §6.8.3 item 7),
/// so the plan is made from them as they are: a node under a point is that node, a point on a
/// street is a point of its axis, anything else is free (the sim snaps nothing). A piece that does
/// not validate (a red ghost, or too close to a road) is not sent and `tool` goes back to
/// `before`, so the chain and its points are not lost. The chain continues from the end.
pub fn chain_commit(
    view: Option<&SimView>,
    tool: &mut RoadTool,
    before: RoadTool,
    req: RoadPathRequest,
    free: bool,
) -> Option<BuildPlan> {
    // The path the ghost showed: quantized by the zone lattice like the ghost's, then resolved to
    // anchors; `free` (snaps off) places it as it was clicked.
    let req = match view {
        Some(v) if !free => quantized(v, req, false).0,
        _ => req,
    };
    let plan = match view {
        Some(v) => {
            let (plan, at) = plan_for(v, &req)?;
            let plan = through_nodes(v, plan);
            let req = RoadPathRequest {
                path: with_vertices(&req.path, &at),
                ..req
            };
            let _ = &req;
            let bad = matches!(preview_plan(v, &plan), Some(Err(_))) || too_close_plan(v, &plan);
            if bad {
                *tool = before;
                return None;
            }
            if let Some(end) = at.last() {
                tool.points = vec![*end];
            }
            plan
        }
        None => {
            if let Some(end) = path_end(&req.path) {
                tool.points = vec![end];
            }
            plan_from_anchors(&req, &anchors_free(&req))
        }
    };
    Some(plan)
}

/// Where an anchor of a plan stands in the plane: the node, the point of the street, the free
/// point. `None` for an anchor the view does not know.
pub fn anchor_pos(view: &SimView, anchor: &Anchor) -> Option<Vec2> {
    match anchor {
        Anchor::Free(p) => Some(*p),
        Anchor::Node(n) => view.net.node(*n).map(|n| to_xz(n.pos)),
        Anchor::OnSegment(s, t) => view
            .net
            .segment(*s)
            .map(|s| to_xz(s.curve.point(t.clamp(0.0, 1.0)))),
    }
}

/// `path` with its vertices moved to `at` (a control point stays).
fn with_vertices(path: &RoadPath, at: &[Vec2]) -> RoadPath {
    match path {
        RoadPath::Straight { .. } => RoadPath::Straight {
            from: at[0],
            to: at[1],
        },
        RoadPath::Curve { control, .. } => RoadPath::Curve {
            from: at[0],
            control: *control,
            to: at[1],
        },
        RoadPath::Free(_) => RoadPath::Free(at.to_vec()),
    }
}

/// The vertices of a path with its control point, if it has one.
fn path_vertices(path: &RoadPath) -> (Vec<Vec2>, Option<Vec2>) {
    match path {
        RoadPath::Straight { from, to } => (vec![*from, *to], None),
        RoadPath::Curve { from, control, to } => (vec![*from, *to], Some(*control)),
        RoadPath::Free(v) => (v.clone(), None),
    }
}

fn anchors_free(req: &RoadPathRequest) -> Vec<(Anchor, Vec2)> {
    path_vertices(&req.path)
        .0
        .into_iter()
        .map(|p| (Anchor::Free(p), p))
        .collect()
}

/// The plan of a path whose vertices are `anchors` (anchor and where it stands): a straight
/// leg is a piece with its control points on the chord, a curve the cubic of its quadratic.
fn plan_from_anchors(req: &RoadPathRequest, anchors: &[(Anchor, Vec2)]) -> BuildPlan {
    let (_, control) = path_vertices(&req.path);
    let pieces = anchors
        .windows(2)
        .map(|w| {
            let ((fa, a), (ta, b)) = (w[0], w[1]);
            let (c1, c2) = match control {
                Some(c) => (a + (c - a) * (2.0 / 3.0), b + (c - b) * (2.0 / 3.0)),
                None => (a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0)),
            };
            PlanPiece {
                from: fa,
                c1,
                c2,
                to: ta,
            }
        })
        .collect();
    BuildPlan {
        road: req.road,
        elevation: req.elevation,
        pieces,
    }
}

/// How far beyond a road's edge (its half width, sidewalks included) a point is still on the road
/// (BUG-085): a cursor just off the kerb joins the street, like in C:S.
pub const DIRECT_PULL_M: f32 = 4.0;

/// Whether both pieces at the plain cut `id` are straight: then a plan that meets the street there
/// dissolves the cut (`cp_net`), and the tool does not snap to it. A cut beside a curve (the end of
/// a rounded corner's bend) stays a node to snap to: it cannot be dissolved (BUG-085).
fn dissolvable_cut(net: &cp_net::NetGraph, id: cp_net::NodeId) -> bool {
    let straight = |c: &CubicBezier3| {
        let (a, b) = (to_xz(c.p0), to_xz(c.p3));
        let d = (b - a).normalize_or_zero();
        [c.p1, c.p2]
            .iter()
            .all(|p| (to_xz(*p) - a).perp_dot(d).abs() < 0.01)
    };
    cp_net::is_plain_cut(net, id)
        && net.node(id).is_some_and(|n| {
            n.segments
                .iter()
                .all(|&s| net.segment(s).is_some_and(|s| straight(&s.curve)))
        })
}

/// What the road tool puts under the point `p` (§6.8.3 items 1 and 5): through the one
/// resolver's direct hit — a node (inside its junction), else a street within its half width and
/// [`DIRECT_PULL_M`] (its point inside a node's junction is that node) — and nothing else, because
/// the quantizing has already placed the point. `is_end`: an end of the
/// path. A raised road meets only raised nodes (at its ends also ground ones) and no streets; a
/// tunnel its own nodes (at its ends the ground ones, the portals).
fn direct_hit(
    view: &SimView,
    road: RoadDefId,
    elevation: ElevationMode,
    p: Vec2,
    is_end: bool,
) -> (Anchor, Vec2) {
    let (Some(content), net) = (view.content.as_deref(), &*view.net) else {
        return (Anchor::Free(p), p);
    };
    let level_ok = |e: cp_net::Elevation| match elevation {
        ElevationMode::Ground => e == cp_net::Elevation::Ground,
        ElevationMode::Elevated(_) => match e {
            cp_net::Elevation::Elevated | cp_net::Elevation::Bridge => true,
            cp_net::Elevation::Ground => is_end,
            cp_net::Elevation::Tunnel => false,
        },
        ElevationMode::Tunnel(_) => match e {
            cp_net::Elevation::Tunnel => true,
            cp_net::Elevation::Ground => is_end,
            _ => false,
        },
    };
    let mut ids = Vec::new();
    let mut nodes = Vec::new();
    for (id, n) in net
        .nodes()
        .filter(|(id, n)| level_ok(n.elevation) && !dissolvable_cut(net, *id))
    {
        ids.push(id);
        // The junction pull (T-275 spec v2, item 8): a disc round the centre.
        nodes.push(SnapNode::radial(
            to_xz(n.pos),
            crate::lattice::node_pull(net, content, id),
        ));
    }
    let mut sids = Vec::new();
    let mut edges = Vec::new();
    if matches!(elevation, ElevationMode::Ground) {
        for (id, s) in net
            .segments()
            .filter(|(_, s)| s.elevation == cp_net::Elevation::Ground)
        {
            sids.push(id);
            edges.push(SnapEdge {
                curve: s.curve,
                reach: content.road(s.road).width / 2.0 + DIRECT_PULL_M,
            });
        }
    }
    let snap = resolve(
        p,
        &SnapCtx {
            nodes: &nodes,
            edges: &edges,
            guides: &[],
            marks: &[],
            start: None,
            refs: &[],
            snaps: false,
            length_step: false,
        },
    );
    match snap.hit {
        Hit::Node(i) => (Anchor::Node(ids[i]), snap.pos),
        // A point of a street inside a junction is the junction, and so is one closer to a
        // non-dead end of its street than the new road's half width (that node would lie under
        // the new asphalt): a cut there would be refused (BUG-085). Near a dead end the point
        // moves out of its footprint instead (T-266).
        Hit::Edge(i, t) => {
            let reach = (content.road(road).width / 2.0).max(snap::MIN_FOOTPRINT_M);
            let ends = net
                .segment(sids[i])
                .map_or(Vec::new(), |s| vec![s.start, s.end]);
            let dead = |id: &cp_net::NodeId| net.node(*id).is_some_and(|n| n.segments.len() == 1);
            let near_end = ids.iter().enumerate().find(|&(k, id)| {
                ends.contains(id) && !dead(id) && nodes[k].pos.distance(snap.pos) < reach
            });
            match nodes.iter().position(|n| n.contains(snap.pos)) {
                Some(k) => (Anchor::Node(ids[k]), nodes[k].pos),
                None => match near_end {
                    Some((k, id)) => (Anchor::Node(*id), nodes[k].pos),
                    None => {
                        let moved = cp_net::out_of_end_footprint(net, content, sids[i], t);
                        (
                            Anchor::OnSegment(sids[i], moved),
                            to_xz(edges[i].curve.point(moved)),
                        )
                    }
                },
            }
        }
        Hit::Free => (Anchor::Free(p), p),
    }
}

/// The plan of `req` as it stands (§6.8.3 item 1) with the anchors of its vertices: the plan and
/// where each vertex ended up. `None` without content.
pub fn plan_for(view: &SimView, req: &RoadPathRequest) -> Option<(BuildPlan, Vec<Vec2>)> {
    view.content.as_deref()?;
    let (vertices, _) = path_vertices(&req.path);
    let last = vertices.len().saturating_sub(1);
    let anchors: Vec<(Anchor, Vec2)> = vertices
        .iter()
        .enumerate()
        .map(|(i, &p)| direct_hit(view, req.road, req.elevation, p, i == 0 || i == last))
        .collect();
    let at = anchors.iter().map(|a| a.1).collect();
    Some((plan_from_anchors(req, &anchors), at))
}

/// Validates `plan` against the snapshot with the same pure function `BuildPlan` is applied
/// with. `None` when the view has no terrain or content.
pub fn preview_plan(
    view: &SimView,
    plan: &BuildPlan,
) -> Option<Result<RoadBuildPlan, Vec<RoadBuildError>>> {
    let terrain = view.terrain.as_deref()?;
    let content = view.content.as_deref()?;
    Some(validate_build_plan(
        &view.net,
        terrain,
        &view.tiles,
        content,
        plan,
    ))
}

/// Validates `req` the way the command will: its vertices resolved to anchors (a node, a point
/// of a street, or free), then the plan applied literally. `None` when the view has no terrain or
/// content (nothing to validate against).
pub fn preview(
    view: &SimView,
    req: &RoadPathRequest,
) -> Option<Result<RoadBuildPlan, Vec<RoadBuildError>>> {
    preview_plan(view, &plan_for(view, req)?.0)
}

/// `req` with its path quantized to the 8 m step and the joins of §6.7 (`alt`: free, no joins and
/// no step), plus what the quantizing snapped. This is the zone-lattice guide of the resolver:
/// the length jumps to the cell boundaries of the streets beside it. Without content there is
/// nothing to look up and `req` comes back as it is.
pub fn quantized(view: &SimView, req: RoadPathRequest, alt: bool) -> (RoadPathRequest, SnapReport) {
    let Some(content) = view.content.as_deref() else {
        return (req, SnapReport::default());
    };
    let marks = crate::lattice::path_marks(view, &req.path);
    let cx = crate::lattice::LatticeCtx {
        graph: &view.net,
        content,
        alt,
        marks: &marks,
        edge_offset: content.road(req.road).width / 2.0,
    };
    // A vertex on or beside a road is that road (BUG-085): the lattice never moves it off.
    let (vertices, _) = path_vertices(&req.path);
    let pinned: Vec<Option<Vec2>> = vertices
        .iter()
        .map(|&p| {
            let hit = direct_hit(view, req.road, req.elevation, p, true);
            (!matches!(hit.0, Anchor::Free(_))).then_some(hit.1)
        })
        .collect();
    // A vertex that is a node goes into the lattice as the node, and so does one the lattice puts
    // where the plan makes it a node (inside a junction): the lattice then aims the legs from
    // where they will start, not from a point beside the node (BUG-086).
    let node_at = |p: Vec2| match direct_hit(view, req.road, req.elevation, p, true) {
        (Anchor::Node(_), at) => Some(at),
        _ => None,
    };
    let mut input: Vec<Vec2> = vertices.iter().map(|&p| node_at(p).unwrap_or(p)).collect();
    let (mut path, mut report) =
        crate::lattice::quantize_path(&with_request_vertices(&req, &input), &cx);
    for _ in 0..2 {
        let (out, _) = path_vertices(&path);
        if out.len() != input.len() {
            break;
        }
        let mut moved = false;
        for (i, &q) in out.iter().enumerate() {
            if let Some(n) = node_at(q)
                && n.distance(q) > 1e-3
                && n.distance(input[i]) > 1e-3
            {
                input[i] = n;
                moved = true;
            }
        }
        if !moved {
            break;
        }
        (path, report) = crate::lattice::quantize_path(&with_request_vertices(&req, &input), &cx);
    }
    let (mut at, _) = path_vertices(&path);
    if pinned.iter().all(Option::is_none) || at.len() != pinned.len() {
        return (RoadPathRequest { path, ..req }, report);
    }
    // A vertex the lattice keeps on a road (a tick of the street, say) stays where the lattice
    // put it, unless that makes the plan refused where the vertex as aimed is not; one it moved
    // off the road goes back onto the road.
    let mut kept = false;
    let mut aimed = at.clone();
    for ((a, b), p) in at.iter_mut().zip(aimed.iter_mut()).zip(&pinned) {
        if let Some(p) = p {
            *b = *p;
            if matches!(
                direct_hit(view, req.road, req.elevation, *a, true).0,
                Anchor::Free(_)
            ) {
                *a = *p;
            } else {
                kept |= a.distance(*p) > 1e-3;
            }
        }
    }
    if kept {
        let refused = |at: &[Vec2]| {
            let r = RoadPathRequest {
                path: with_vertices(&path, at),
                ..req.clone()
            };
            matches!(preview(view, &r), Some(Err(_)))
        };
        if refused(&at) && !refused(&aimed) {
            at = aimed;
        }
    }
    let path = with_vertices(&path, &at);
    (RoadPathRequest { path, ..req }, report)
}

/// `req` with the vertices of its path moved to `at`.
fn with_request_vertices(req: &RoadPathRequest, at: &[Vec2]) -> RoadPathRequest {
    RoadPathRequest {
        path: with_vertices(&req.path, at),
        ..req.clone()
    }
}

/// The cursor of the road tool after the one resolver (§6.8.3 items 1, 5 and 7): where the ghost
/// ends, where a click puts the point, and the plan the click sends.
#[derive(Clone, Debug, PartialEq)]
pub struct Cursor {
    /// The cursor after the block guide, before the zone lattice: what a click stores (the
    /// lattice is applied to the whole path, with its first point).
    pub cursor: Vec2,
    /// The resolved point (the last vertex of `path`).
    pub point: Vec2,
    /// The path with its vertices resolved: nothing is left to snap.
    pub path: Option<RoadPath>,
    /// The plan of `path`.
    pub plan: Option<BuildPlan>,
    /// What snapped the point.
    pub snap: SnapReport,
    /// The 8-cell block guide, if it moved the cursor.
    pub block: Option<crate::lattice::BlockGuide>,
}

/// Resolves the cursor `hover` of `tool` once. `free`: snaps off (`Alt`, the button): no block
/// guide, no joins, no length step, only a direct hit on a node or a street within its width.
/// The ghost and the click both call this, so the click is what the ghost showed.
pub fn resolve_cursor(view: &SimView, tool: &RoadTool, hover: Vec2, free: bool) -> Option<Cursor> {
    tool.road?;
    let (hover, block) = if free {
        (hover, None)
    } else {
        // No 8-cell block guide (T-275 spec v2: the sections place a street).
        (hover, None)
    };
    // The path with the cursor; with nothing clicked yet, a probe leg east of the first point:
    // the rules for a vertex do not depend on the leg.
    let drawing = tool.path_with_hover(hover);
    let req = tool.request(drawing.clone().unwrap_or(RoadPath::Straight {
        from: hover,
        to: hover + Vec2::X * 40.0,
    }))?;
    let (req, step) = if free {
        (req, SnapReport::default())
    } else {
        quantized(view, req, false)
    };
    let (plan, at) = plan_for(view, &req)?;
    let k = if drawing.is_some() { at.len() - 1 } else { 0 };
    let point = at[k];
    // The path with the vertices where the anchors put them (a point on a street is on its axis).
    let path = drawing.map(|_| with_vertices(&req.path, &at));
    let plan = path.as_ref().map(|path| {
        plan_from_anchors(
            &RoadPathRequest {
                path: path.clone(),
                ..req.clone()
            },
            &plan_anchors(&plan, &at),
        )
    });
    let plan = plan.map(|p| through_nodes(view, p));
    let mut snap = step;
    if let Some(plan) = &plan
        && let Some(last) = plan.pieces.last()
    {
        match last.to {
            Anchor::Node(_) => snap.kind = SnapReportKind::Node,
            Anchor::OnSegment(..) => snap.kind = SnapReportKind::Segment,
            Anchor::Free(_) => {}
        }
    }
    Some(Cursor {
        cursor: hover,
        point,
        path,
        plan,
        snap,
        block,
    })
}

/// `plan` with every ground node its straight pieces run through (within [`THROUGH_M`] of the
/// axis) made a vertex: a street drawn across the dead ends of streets that stop on its line joins
/// them, instead of being refused for a junction on a road it does not join (T-275: the street
/// that closes the long streets' ends).
pub fn through_nodes(view: &SimView, plan: BuildPlan) -> BuildPlan {
    if !matches!(plan.elevation, ElevationMode::Ground) {
        return plan;
    }
    let net = &*view.net;
    let mut pieces = Vec::with_capacity(plan.pieces.len());
    for piece in &plan.pieces {
        let (Some(a), Some(b)) = (anchor_pos(view, &piece.from), anchor_pos(view, &piece.to))
        else {
            pieces.push(*piece);
            continue;
        };
        let chord = b - a;
        let len = chord.length();
        let straight = len > 1e-3
            && [piece.c1, piece.c2]
                .iter()
                .all(|c| (*c - a).perp_dot(chord / len).abs() < 0.01);
        if !straight {
            pieces.push(*piece);
            continue;
        }
        let u = chord / len;
        let mut on: Vec<(f32, cp_net::NodeId, Vec2)> = net
            .nodes()
            .filter(|(id, n)| {
                n.elevation == cp_net::Elevation::Ground
                    && piece.from != Anchor::Node(*id)
                    && piece.to != Anchor::Node(*id)
            })
            .filter_map(|(id, n)| {
                let p = to_xz(n.pos);
                let s = (p - a).dot(u);
                let off = (p - a).perp_dot(u).abs();
                (off < THROUGH_M
                    && s > cp_net::build::ZONE_CELL
                    && s < len - cp_net::build::ZONE_CELL)
                    .then_some((s, id, p))
            })
            .collect();
        if on.is_empty() {
            pieces.push(*piece);
            continue;
        }
        on.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut from = (piece.from, a);
        let stops = on
            .into_iter()
            .map(|(_, id, p)| (Anchor::Node(id), p))
            .chain(std::iter::once((piece.to, b)));
        for (to, p) in stops {
            let (fa, fp) = from;
            pieces.push(PlanPiece {
                from: fa,
                c1: fp.lerp(p, 1.0 / 3.0),
                c2: fp.lerp(p, 2.0 / 3.0),
                to,
            });
            from = (to, p);
        }
    }
    BuildPlan { pieces, ..plan }
}

/// How near (metres) a node must be to a straight piece's axis to become its vertex.
const THROUGH_M: f32 = 0.5;

/// The anchors of a plan with the points they stand at, in vertex order.
fn plan_anchors(plan: &BuildPlan, at: &[Vec2]) -> Vec<(Anchor, Vec2)> {
    let mut out = Vec::with_capacity(at.len());
    for (i, piece) in plan.pieces.iter().enumerate() {
        if i == 0 {
            out.push((piece.from, at[0]));
        }
        out.push((piece.to, at[i + 1]));
    }
    out
}

/// The verdict shown when a new road runs less than one cell from another (§6.7, T-124).
pub const TOO_CLOSE: &str = "too close to another road: leave a cell (8 m) or connect";

/// Does the path of `req` pass validation but run closer than one cell to an existing road? The
/// tool then draws a red ghost and sends no command; the command itself does not check this.
pub fn too_close(view: &SimView, req: &RoadPathRequest) -> bool {
    plan_for(view, req).is_some_and(|(plan, _)| too_close_plan(view, &plan))
}

/// [`too_close`] for a plan.
pub fn too_close_plan(view: &SimView, plan: &BuildPlan) -> bool {
    let (Some(content), Some(Ok(built))) = (view.content.as_deref(), preview_plan(view, plan))
    else {
        return false;
    };
    !clearance_report(&view.net, content, &built).is_empty()
}

/// What a marker of the road tool stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerKind {
    /// An 8-cell line of a street's cells: where the near edge of a cross street leaves a block
    /// of 8 cells (drawn as a diamond).
    Control,
    /// A cell line, shown while the cursor is within a cell of a road edge (a small cross).
    Tick,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadMarker {
    pub pos: Vec2,
    pub kind: MarkerKind,
}

/// The markers the road tool draws around the cursor `hover`, all from the cell lines of the
/// zone layout (T-266, §6.8.3 item 3): the 8-cell lines of every street within [`GUIDE_RADIUS`]
/// of it and, when the cursor is within one cell of a street's edge, every cell line of that
/// street. Derived from the net; nothing is stored.
///
/// [`GUIDE_RADIUS`]: cp_net::build::GUIDE_RADIUS
pub fn road_markers(view: &SimView, hover: Vec2) -> Vec<RoadMarker> {
    let Some(content) = view.content.as_deref() else {
        return Vec::new();
    };
    let graph = &*view.net;
    let mut near_edge: Option<(f32, cp_net::SegmentId)> = None;
    // The streets within the guide radius show all their control lines.
    let mut near = std::collections::BTreeSet::new();
    for (id, s) in graph.segments() {
        if s.elevation != cp_net::Elevation::Ground
            || s.curve.bounds_distance_xz(hover) > cp_net::build::GUIDE_RADIUS + 1e-3
        {
            continue;
        }
        let (_, d) = s.curve.nearest_xz(hover);
        let reach = content.road(s.road).width * 0.5 + cp_net::build::ZONE_CELL;
        if d <= reach && near_edge.is_none_or(|(bd, _)| d < bd) {
            near_edge = Some((d, id));
        }
        if d <= cp_net::build::GUIDE_RADIUS {
            near.insert(id);
        }
    }
    let _ = near_edge;
    // The section marks of the streets near the cursor (T-275 spec v2): where a street's end or
    // a cross street's centre snaps, every 10 cells from either end of the street's run.
    let mut out: Vec<RoadMarker> = Vec::new();
    for id in near {
        for (_, pos, _) in crate::lattice::section_marks(graph, id) {
            if !out.iter().any(|o| o.pos.distance(pos) < 0.05) {
                out.push(RoadMarker {
                    pos,
                    kind: MarkerKind::Control,
                });
            }
        }
    }
    out
}

/// Distance between direction chevrons along a one-way ghost, metres.
pub const CHEVRON_SPACING: f32 = 16.0;
/// Lift of the chevrons over the curve, metres (above the 0.3 m ghost band).
const CHEVRON_LIFT: f32 = 0.7;

/// Do all the car lanes of `def` run `Forward` (and are there any)?
pub fn road_is_one_way(def: &RoadDef) -> bool {
    def.is_one_way()
}

/// The direction chevrons of a one-way piece: one every [`CHEVRON_SPACING`] metres, so
/// `floor(length / 16)` of them, centred in their 16 m cell. Each chevron is three gizmo
/// segments: the two arms of the arrowhead and a short stem behind its tip, pointing along
/// the curve from `start` to `end`.
pub fn chevron_lines(curve: &CubicBezier3, length: f32) -> Vec<[Vec3; 2]> {
    let n = (length / CHEVRON_SPACING).floor().max(0.0) as usize;
    let table = curve.arc_table();
    let lift = Vec3::Y * CHEVRON_LIFT;
    let mut out = Vec::with_capacity(n * 3);
    for i in 0..n {
        let s = ((i as f32 + 0.5) * CHEVRON_SPACING).min(table.length());
        let t = table.s_to_t(s);
        let p = curve.point(t) + lift;
        let flat = Vec3::new(curve.tangent(t).x, 0.0, curve.tangent(t).z);
        let Some(dir) = flat.try_normalize() else {
            continue;
        };
        let side = Vec3::new(-dir.z, 0.0, dir.x);
        let tip = p + dir * 1.5;
        out.push([p - dir * 1.0 + side * 1.5, tip]);
        out.push([p - dir * 1.0 - side * 1.5, tip]);
        out.push([p - dir * 1.8, tip]);
    }
    out
}

/// The tool's label for a snap: `90°`, `45°`, `∥` or `⊥`; steps and plain snaps show none.
pub fn snap_label(kind: SnapReportKind) -> Option<String> {
    match kind {
        SnapReportKind::KeyAngle(d) => Some(format!("{d}°")),
        SnapReportKind::Guide { perpendicular } => {
            Some(if perpendicular { "⊥" } else { "∥" }.to_string())
        }
        SnapReportKind::ControlLine(d) => Some(format!("⊥ {d}")),
        _ => None,
    }
}

/// Cost as a money string: cents → `$1,234.56`.
pub fn format_money(cost: Cost) -> String {
    let cents = cost.0;
    let sign = if cents < 0 { "-" } else { "" };
    let cents = cents.unsigned_abs();
    let dollars = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in dollars.chars().enumerate() {
        if i > 0 && (dollars.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}${grouped}.{:02}", cents % 100)
}

/// Points along a curve, about every `step` metres.
pub fn sample_curve(c: &CubicBezier3, step: f32) -> Vec<Vec3> {
    let len = c.arc_table().length().max(step);
    let n = (len / step).ceil() as usize;
    (0..=n).map(|i| c.point(i as f32 / n as f32)).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn lowering_from_the_ground_goes_into_a_tunnel_and_raising_comes_back() {
        let mut tool = RoadTool::default();
        assert_eq!(tool.elevation(), ElevationMode::Ground);
        tool.lower();
        assert_eq!(tool.elevation(), ElevationMode::Tunnel(ROAD_TUNNEL_DEPTH));
        tool.raise();
        assert_eq!(tool.elevation(), ElevationMode::Ground);
        tool.raise();
        assert_eq!(tool.elevation(), ElevationMode::Elevated(ELEVATION_STEP));
        tool.lower();
        tool.lower();
        assert_eq!(tool.elevation(), ElevationMode::Tunnel(ROAD_TUNNEL_DEPTH));
    }

    use super::*;

    fn tool(mode: RoadMode) -> RoadTool {
        RoadTool {
            mode,
            road: Some(RoadDefId(0)),
            ..Default::default()
        }
    }

    #[test]
    fn snap_labels_show_key_angles_and_guides_only() {
        assert_eq!(
            snap_label(SnapReportKind::KeyAngle(90)).as_deref(),
            Some("90°")
        );
        assert_eq!(
            snap_label(SnapReportKind::KeyAngle(45)).as_deref(),
            Some("45°")
        );
        assert_eq!(
            snap_label(SnapReportKind::Guide {
                perpendicular: false
            })
            .as_deref(),
            Some("∥")
        );
        assert_eq!(
            snap_label(SnapReportKind::Guide {
                perpendicular: true
            })
            .as_deref(),
            Some("⊥")
        );
        assert_eq!(
            snap_label(SnapReportKind::ControlLine(64)).as_deref(),
            Some("⊥ 64")
        );
        assert_eq!(snap_label(SnapReportKind::CellStep(48)), None);
        assert_eq!(snap_label(SnapReportKind::Step(15)), None);
        assert_eq!(snap_label(SnapReportKind::Node), None);
    }

    #[test]
    fn money_is_formatted_from_cents() {
        assert_eq!(format_money(Cost(0)), "$0.00");
        assert_eq!(format_money(Cost(5)), "$0.05");
        assert_eq!(format_money(Cost(123_456)), "$1,234.56");
        assert_eq!(format_money(Cost(100_000_000)), "$1,000,000.00");
        assert_eq!(format_money(Cost(-250)), "-$2.50");
    }

    #[test]
    fn elevation_levels_step_and_clamp() {
        let mut t = tool(RoadMode::Straight);
        assert_eq!(t.elevation(), ElevationMode::Ground);
        // Lowering from the ground goes under it (T-173), raising comes back.
        t.lower();
        assert_eq!(t.level, 0);
        assert_eq!(t.elevation(), ElevationMode::Tunnel(ROAD_TUNNEL_DEPTH));
        t.raise();
        assert_eq!(t.elevation(), ElevationMode::Ground);
        t.raise();
        assert_eq!(t.elevation(), ElevationMode::Elevated(ELEVATION_STEP));
        for _ in 0..10 {
            t.raise();
        }
        assert_eq!(t.level, MAX_ELEVATION_LEVEL);
        t.lower();
        assert_eq!(t.level, MAX_ELEVATION_LEVEL - 1);
    }

    /// T-164: the metro track tool draws in a tunnel; the height keys leave it alone.
    #[test]
    fn a_tunnel_tool_sends_tunnel_requests_and_ignores_the_height_keys() {
        let mut t = tool(RoadMode::Straight);
        t.tunnel_depth = Some(12.0);
        t.raise();
        assert_eq!(t.level, 0);
        assert_eq!(t.elevation(), ElevationMode::Tunnel(12.0));
        assert!(t.click(Vec2::new(0.0, 0.0)).is_none());
        let req = t.click(Vec2::new(64.0, 0.0)).unwrap();
        assert_eq!(req.elevation, ElevationMode::Tunnel(12.0));
    }

    #[test]
    fn straight_takes_two_clicks() {
        let mut t = tool(RoadMode::Straight);
        assert!(t.path_with_hover(Vec2::ONE).is_none());
        assert!(t.click(Vec2::new(1.0, 2.0)).is_none());
        assert_eq!(
            t.path_with_hover(Vec2::new(5.0, 2.0)),
            Some(RoadPath::Straight {
                from: Vec2::new(1.0, 2.0),
                to: Vec2::new(5.0, 2.0)
            })
        );
        let req = t.click(Vec2::new(9.0, 2.0)).unwrap();
        assert_eq!(
            req.path,
            RoadPath::Straight {
                from: Vec2::new(1.0, 2.0),
                to: Vec2::new(9.0, 2.0)
            }
        );
        assert_eq!(req.snap, SnapOptions::default());
        assert_eq!(
            t.points,
            [Vec2::new(9.0, 2.0)],
            "the chain goes on from the last node"
        );
    }

    #[test]
    fn curve_takes_three_clicks_start_control_end() {
        let mut t = tool(RoadMode::Curve);
        assert!(t.click(Vec2::new(0.0, 0.0)).is_none());
        // After the first click the ghost is a straight line; after the second a curve.
        assert!(matches!(
            t.path_with_hover(Vec2::new(10.0, 0.0)),
            Some(RoadPath::Straight { .. })
        ));
        assert!(t.click(Vec2::new(50.0, 0.0)).is_none());
        assert_eq!(
            t.path_with_hover(Vec2::new(50.0, 50.0)),
            Some(RoadPath::Curve {
                from: Vec2::new(0.0, 0.0),
                control: Vec2::new(50.0, 0.0),
                to: Vec2::new(50.0, 50.0)
            })
        );
        let req = t.click(Vec2::new(50.0, 50.0)).unwrap();
        assert!(matches!(req.path, RoadPath::Curve { .. }));
    }

    #[test]
    fn free_mode_collects_points_until_finished() {
        let mut t = tool(RoadMode::Free);
        for p in [(0.0, 0.0), (30.0, 0.0), (30.0, 30.0)] {
            assert!(t.click(Vec2::new(p.0, p.1)).is_none());
        }
        assert_eq!(
            t.path_with_hover(Vec2::new(60.0, 30.0)),
            Some(RoadPath::Free(vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(30.0, 0.0),
                Vec2::new(30.0, 30.0),
                Vec2::new(60.0, 30.0)
            ]))
        );
        // Clicking on the last point again finishes.
        let req = t.click(Vec2::new(30.0, 30.0)).unwrap();
        assert_eq!(
            req.path,
            RoadPath::Free(vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(30.0, 0.0),
                Vec2::new(30.0, 30.0)
            ])
        );
        assert_eq!(t.points, [Vec2::new(30.0, 30.0)], "the chain goes on");
        // Enter with fewer than two points does nothing.
        t.cancel();
        t.click(Vec2::ZERO);
        assert!(t.finish_free().is_none());
        t.click(Vec2::new(20.0, 0.0));
        assert!(t.finish_free().is_some());
    }

    /// T-176: after a piece the next one starts at its last point; Esc ends the chain.
    #[test]
    fn a_chain_continues_from_the_last_point_in_every_mode_and_cancel_ends_it() {
        let mut t = tool(RoadMode::Straight);
        t.click(Vec2::new(0.0, 0.0));
        t.click(Vec2::new(40.0, 0.0)).unwrap();
        let next = t.click(Vec2::new(40.0, 40.0)).unwrap();
        assert_eq!(
            next.path,
            RoadPath::Straight {
                from: Vec2::new(40.0, 0.0),
                to: Vec2::new(40.0, 40.0)
            },
            "one click: start = the last node"
        );
        t.cancel();
        assert!(t.points.is_empty(), "Esc ends the chain");
        assert!(t.click(Vec2::new(90.0, 90.0)).is_none(), "a fresh start");

        // A curve: the next piece starts at the end and takes control and end again.
        let mut c = tool(RoadMode::Curve);
        for p in [(0.0, 0.0), (30.0, 0.0), (30.0, 30.0)] {
            c.click(Vec2::new(p.0, p.1));
        }
        assert_eq!(c.points, [Vec2::new(30.0, 30.0)]);
        assert!(c.click(Vec2::new(30.0, 60.0)).is_none());
        let req = c.click(Vec2::new(60.0, 60.0)).unwrap();
        assert!(matches!(
            req.path,
            RoadPath::Curve { from, .. } if from == Vec2::new(30.0, 30.0)
        ));

        // Free: finishing keeps the last point as the start of the next polyline.
        let mut f = tool(RoadMode::Free);
        for p in [(0.0, 0.0), (30.0, 0.0)] {
            f.click(Vec2::new(p.0, p.1));
        }
        f.finish_free().unwrap();
        f.click(Vec2::new(30.0, 30.0));
        let req = f.finish_free().unwrap();
        assert_eq!(
            req.path,
            RoadPath::Free(vec![Vec2::new(30.0, 0.0), Vec2::new(30.0, 30.0)])
        );
    }

    /// A piece that does not validate is not sent and leaves the chain as it was; a good one
    /// moves the chain on to its end.
    #[test]
    fn a_red_piece_does_not_drop_the_chain() {
        use std::path::Path;
        use std::sync::Arc;
        let root = env!("CARGO_MANIFEST_DIR");
        let mut sim = cp_sim::SimRunner::new(1);
        sim.set_terrain(Arc::new(
            cp_terrain::Terrain::load(&Path::new(root).join("../../assets/maps/flatlands"))
                .unwrap(),
        ));
        let db = Arc::new(
            cp_content::ContentDb::load_dir(Path::new(root).join("../../assets/content")).unwrap(),
        );
        let road = db.road_id("basic_2l").unwrap();
        sim.set_content(db);
        let view = sim.view();
        let mut t = RoadTool {
            road: Some(road),
            ..Default::default()
        };
        // Outside the bought land: refused.
        t.click(Vec2::new(100.0, 100.0));
        let before = t.clone();
        let req = t.click(Vec2::new(200.0, 100.0)).unwrap();
        assert!(chain_commit(Some(&view), &mut t, before.clone(), req, false).is_none());
        assert_eq!(t, before);
        // On owned land: sent, and the chain goes on from the end.
        t.cancel();
        t.click(Vec2::new(5000.0, 5000.0));
        let before = t.clone();
        let req = t.click(Vec2::new(5096.0, 5000.0)).unwrap();
        let sent = chain_commit(Some(&view), &mut t, before, req, false).expect("valid");
        let cp_net::Anchor::Free(to) = sent.pieces.last().unwrap().to else {
            panic!("a free end")
        };
        assert_eq!(t.points, [to]);
    }

    #[test]
    fn changing_mode_or_cancelling_drops_the_drawing() {
        let mut t = tool(RoadMode::Curve);
        t.click(Vec2::ZERO);
        t.set_mode(RoadMode::Free);
        assert!(t.points.is_empty());
        t.click(Vec2::ZERO);
        t.cancel();
        assert!(t.points.is_empty());
    }

    #[test]
    fn nothing_is_built_without_a_road_type() {
        let mut t = RoadTool::default();
        assert!(t.click(Vec2::ZERO).is_none());
        assert!(t.points.is_empty());
    }
}

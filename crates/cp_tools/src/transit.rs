//! The bus-line tool (T-157): the click state machine, the stop under the cursor and the ghost
//! of the route. Everything is a pure function of the [`SimView`] (its `transit` snapshot has
//! the stops, the lines and the lane graph) and the tool; the rules and the route are the ones
//! the command uses (`check_road_stop`, `plan_line`).
//!
//! * a click on a road puts a new stop there (the side is the one the cursor is on);
//! * a click on an existing stop adds it;
//! * a click on the first stop closes the ring and sends `CreateLine`;
//! * Backspace takes the last stop away, Esc drops the drawing.

use bevy_math::Vec2;
use cp_core::{Arena, Cost};
use cp_locale::{tr, trf};
use cp_sim::{SimCommand, SimView};
use cp_traffic::{Access, Side};
use cp_transit::{
    LineId, LineRoute, MAX_LINE_STOPS, Stop, StopId, StopKind, StopRef, TransitCommand,
    TransitMode, check_road_stop, next_line_color, plan_line, plan_metro, rules::pavement_offset,
};

use crate::bulldozer::pick_segment;

/// How far from a stop's pavement point a click still means that stop, metres.
pub const STOP_PICK_RADIUS: f32 = 5.0;
/// How far from a road's centre line a click still means the road, metres.
pub const ROAD_PICK_RADIUS: f32 = 12.0;

/// A stop placed by the tool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub stop: StopRef,
    pub pos: Vec2,
}

/// State of the tool: the stops of the line being drawn, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineTool {
    pub stops: Vec<Placed>,
    /// What the line is for: a bus line takes road stops, a metro line only stations (T-164).
    pub mode: TransitMode,
    /// The line whose stops are being redrawn: closing the ring sends `EditLine` for it
    /// instead of `CreateLine` (T-160b).
    pub editing: Option<LineId>,
}

/// What is under the cursor.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Nothing the tool can use.
    Nothing,
    /// An existing stop (or the pending first one: `closes`).
    Stop { placed: Placed, closes: bool },
    /// A new stop that the rules allow.
    New(Placed),
    /// A spot on a road where the rules say no.
    Refused { pos: Vec2, why: String },
}

/// What a click did.
#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    Nothing,
    Added,
    /// The ring is closed: send this.
    Create(SimCommand),
    Refused(String),
}

/// The ghost: what the tool shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineGhostInfo {
    pub stops: Vec<Vec2>,
    pub hover: Option<(Vec2, bool)>,
    /// The route as a polyline, with the legs that have a path.
    pub route: Vec<Vec2>,
    /// Height of each route point for a route on the track; empty on the street.
    pub heights: Vec<f32>,
    pub color: u8,
    pub length_m: f32,
    pub cycle_s: f32,
    /// Price of the new stops, if the line were closed now.
    pub cost: Cost,
    /// The click on the hover would close the ring.
    pub closes: bool,
    /// Player-facing problem (the rule that refuses the hover, a leg without a way).
    pub error: Option<String>,
}

fn scratch_with(view: &SimView, tool: &LineTool, upto: usize) -> Arena<Stop> {
    let mut scratch = view.transit.net.stops.clone();
    for p in &tool.stops[..upto.min(tool.stops.len())] {
        if let StopRef::NewRoad { seg, s, side } = p.stop {
            scratch.insert(Stop {
                pos: p.pos,
                side,
                kind: StopKind::Road { seg, s },
                lines: Vec::new(),
            });
        }
    }
    scratch
}

/// The stop or the place for a stop at `p`.
pub fn target_at(view: &SimView, tool: &LineTool, p: Vec2) -> Target {
    // The first stop of the drawing closes the ring.
    if tool.stops.len() >= 2
        && let Some(first) = tool.stops.first()
        && first.pos.distance(p) <= STOP_PICK_RADIUS
    {
        return Target::Stop {
            placed: *first,
            closes: true,
        };
    }
    // The nearest existing stop.
    let mut best: Option<(f32, StopId)> = None;
    for (id, st) in view.transit.net.stops.iter() {
        let d = st.pos.distance(p);
        if matches!(st.kind, StopKind::Station { .. }) != (tool.mode != TransitMode::Bus) {
            continue;
        }
        if d <= STOP_PICK_RADIUS && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, id));
        }
    }
    if let Some((_, id)) = best {
        let pos = view.transit.net.stops.get(id).map_or(p, |s| s.pos);
        return Target::Stop {
            placed: Placed {
                stop: StopRef::Existing(id),
                pos,
            },
            closes: false,
        };
    }
    // A metro line stops at stations only: a click on a road is nothing.
    if tool.mode != TransitMode::Bus {
        return Target::Nothing;
    }
    let (Some(seg_id), Some(db)) = (
        pick_segment(&view.net, p, ROAD_PICK_RADIUS),
        view.content.as_deref(),
    ) else {
        return Target::Nothing;
    };
    let Some(seg) = view.net.segment(seg_id) else {
        return Target::Nothing;
    };
    let (t, _) = seg.curve.nearest_xz(p);
    let s = (seg.curve.arc_table().t_to_s(t) * 2.0).round() / 2.0;
    // The cursor's side of the road: left of the direction of travel is `(dz, -dx)`.
    let on = cp_core::curve::to_xz(seg.curve.point(t));
    let dir = cp_core::curve::to_xz(seg.curve.derivative(t)).normalize_or_zero();
    let left = Vec2::new(dir.y, -dir.x);
    let side = if (p - on).dot(left) >= 0.0 {
        Side::Left
    } else {
        Side::Right
    };
    let scratch = scratch_with(view, tool, tool.stops.len());
    match check_road_stop(
        &view.net,
        db,
        &scratch,
        tool.mode.class(),
        seg_id,
        s,
        side,
        None,
    ) {
        Ok(pos) => Target::New(Placed {
            stop: StopRef::NewRoad {
                seg: seg_id,
                s,
                side,
            },
            pos,
        }),
        Err(e) => {
            // Where the stop would be drawn, if the road has pavement there.
            let pos = pavement_offset(db, seg, side)
                .map_or(on, |o| cp_transit::rules::pavement_point(seg, s, o));
            Target::Refused {
                pos,
                why: stop_error_text(&e),
            }
        }
    }
}

impl LineTool {
    pub fn cancel(&mut self) {
        self.stops.clear();
        self.editing = None;
    }

    /// Starts redrawing `line`: its stops are placed, the player adds, drops or reorders
    /// (undo, then click) and closes the ring again. `None` if the line is gone.
    pub fn begin_edit(&mut self, view: &SimView, line: LineId) -> bool {
        let net = &view.transit.net;
        let Some(l) = net.lines.get(line) else {
            return false;
        };
        self.stops = l
            .stops
            .iter()
            .filter_map(|&s| {
                Some(Placed {
                    stop: StopRef::Existing(s),
                    pos: net.stops.get(s)?.pos,
                })
            })
            .collect();
        self.editing = Some(line);
        self.mode = l.mode;
        true
    }

    /// Takes the last stop away.
    pub fn undo(&mut self) {
        self.stops.pop();
    }

    /// The command that closing the ring now sends.
    pub fn command(&self) -> Option<SimCommand> {
        (self.stops.len() >= 2).then(|| {
            let stops = self.stops.iter().map(|p| p.stop).collect();
            SimCommand::Transit(match self.editing {
                Some(line) => TransitCommand::EditLine { line, stops },
                None => TransitCommand::CreateLine {
                    mode: self.mode,
                    stops,
                },
            })
        })
    }

    /// A click at `p`.
    pub fn click(&mut self, view: &SimView, p: Vec2) -> Click {
        match target_at(view, self, p) {
            Target::Nothing => Click::Nothing,
            Target::Refused { why, .. } => Click::Refused(why),
            Target::Stop { closes: true, .. } => {
                let cmd = self.command().expect("closing needs two stops");
                self.stops.clear();
                self.editing = None;
                Click::Create(cmd)
            }
            Target::Stop { placed, .. } | Target::New(placed) => {
                if self.stops.iter().any(|q| q.stop == placed.stop) {
                    return Click::Nothing;
                }
                if self.stops.len() >= MAX_LINE_STOPS {
                    return Click::Refused(trf!(
                        "a line has at most {n} stops",
                        n = MAX_LINE_STOPS
                    ));
                }
                self.stops.push(placed);
                Click::Added
            }
        }
    }
}

/// The ghost with the cursor at `hover`; `None` when the tool has nothing to show (no stop
/// yet and the cursor is on nothing).
pub fn line_ghost(view: &SimView, tool: &LineTool, hover: Vec2) -> Option<LineGhostInfo> {
    let target = target_at(view, tool, hover);
    let mut info = LineGhostInfo {
        stops: tool.stops.iter().map(|p| p.pos).collect(),
        color: next_line_color(&view.transit.net),
        ..Default::default()
    };
    // The refs the route is planned over: the placed stops and, if it is a new one, the hover.
    let mut refs: Vec<Placed> = tool.stops.clone();
    match &target {
        Target::Nothing => {}
        Target::Refused { pos, why } => {
            info.hover = Some((*pos, false));
            info.error = Some(why.clone());
        }
        Target::Stop {
            placed,
            closes: true,
        } => {
            info.hover = Some((placed.pos, true));
            info.closes = true;
        }
        Target::Stop { placed, .. } | Target::New(placed) => {
            if tool.stops.iter().any(|q| q.stop == placed.stop) {
                info.hover = Some((placed.pos, false));
            } else {
                info.hover = Some((placed.pos, true));
                refs.push(*placed);
            }
        }
    }
    if info.hover.is_none() && tool.stops.is_empty() {
        return None;
    }
    let db = view.content.as_deref()?;
    let new_stops = tool
        .stops
        .iter()
        .filter(|p| matches!(p.stop, StopRef::NewRoad { .. }))
        .count();
    info.cost = Cost::scaled(db.balance().bus_stop_cost, new_stops as f64);
    if refs.len() >= 2 {
        let mut scratch = view.transit.net.stops.clone();
        let mut planned: Vec<(StopId, Access)> = Vec::new();
        for p in &refs {
            match p.stop {
                StopRef::Existing(id) => {
                    if let Some(a) = view.transit.net.access(id) {
                        planned.push((id, a));
                    }
                }
                StopRef::NewRoad { seg, s, side } => {
                    let id = scratch.insert(Stop {
                        pos: p.pos,
                        side,
                        kind: StopKind::Road { seg, s },
                        lines: Vec::new(),
                    });
                    planned.push((
                        id,
                        Access {
                            segment: seg,
                            s,
                            side,
                        },
                    ));
                }
                StopRef::Station(_) => {}
            }
        }
        let (plan, route) = if tool.mode == TransitMode::Metro {
            let plan = plan_metro(&view.transit.graph, &planned);
            let route = LineRoute::of_rail_plan(&view.transit.graph, &plan);
            (plan, route)
        } else {
            let plan = plan_line(&view.transit.graph, tool.mode.class(), &planned);
            let route = LineRoute::of_plan(&view.transit.graph, &plan);
            (plan, route)
        };
        info.route = route.points;
        info.heights = route.heights;
        info.length_m = plan.length_m;
        info.cycle_s = plan.cycle_s;
        if !plan.is_ok() {
            info.error
                .get_or_insert_with(|| tr("no way between two stops"));
        }
    }
    Some(info)
}

/// "320 m · 1 min 05 s round trip" for the verdict line.
pub fn route_summary(info: &LineGhostInfo) -> String {
    let secs = info.cycle_s.round() as u32;
    trf!(
        "{m} m · round trip {time}",
        m = format!("{:.0}", info.length_m),
        time = format!("{}:{:02}", secs / 60, secs % 60)
    )
}

/// Why a stop cannot go here, in the player's language (the English is `e`'s `Display`).
fn stop_error_text(e: &cp_transit::StopError) -> String {
    use cp_transit::{STOP_NODE_CLEARANCE, STOP_SPACING, StopError};
    match e {
        StopError::NearNode => trf!(
            "a stop is at least {m} m from a junction",
            m = STOP_NODE_CLEARANCE
        ),
        StopError::NearStop(_) => trf!("a stop is at least {m} m from the next", m = STOP_SPACING),
        _ => tr(&e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use bevy_math::Vec3;
    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, NetGraph};
    use cp_sim::SimRunner;

    const ROOT: &str = env!("CARGO_MANIFEST_DIR");

    /// A 200 m square of road, ticked once so that the lane graph exists.
    fn town() -> SimRunner {
        let db = Arc::new(ContentDb::load_dir(format!("{ROOT}/../../assets/content")).unwrap());
        let mut sim = SimRunner::new(1);
        sim.set_content(Arc::clone(&db));
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        let c = [
            (0.0, 0.0),
            (200.0, 0.0),
            (200.0, 200.0),
            (0.0, 200.0),
            (0.0, 0.0),
        ];
        for w in c.windows(2) {
            sim.world_mut()
                .resource_mut::<NetGraph>()
                .add_segment(
                    &def,
                    road,
                    CubicBezier3::line(
                        Vec3::new(w[0].0, 0.0, w[0].1),
                        Vec3::new(w[1].0, 0.0, w[1].1),
                    ),
                    Elevation::Ground,
                )
                .unwrap();
        }
        sim.tick_once();
        sim
    }

    /// The pavement of the south side, outside the square (z < 0 is the right of +x travel…
    /// either side will do: the click picks the one it is on).
    fn at(x: f32, z: f32) -> Vec2 {
        Vec2::new(x, z)
    }

    #[test]
    fn three_road_clicks_and_a_click_on_the_first_stop_send_one_create_line() {
        let sim = town();
        let view = sim.view();
        let mut tool = LineTool::default();
        for p in [at(60.0, 3.0), at(197.0, 100.0), at(100.0, 197.0)] {
            assert_eq!(tool.click(&view, p), Click::Added, "{p:?}");
        }
        assert_eq!(tool.stops.len(), 3);
        // The first stop sits where the first click put it (on the pavement).
        let first = tool.stops[0].pos;
        let Click::Create(SimCommand::Transit(TransitCommand::CreateLine { mode, stops })) =
            tool.click(&view, first)
        else {
            panic!("the click on the first stop closes the ring");
        };
        assert_eq!(mode, TransitMode::Bus);
        assert_eq!(stops.len(), 3);
        assert!(stops.iter().all(|s| matches!(s, StopRef::NewRoad { .. })));
        assert!(tool.stops.is_empty(), "ready for the next line");
    }

    #[test]
    fn a_click_near_a_junction_adds_no_stop_and_backspace_takes_the_last_one_away() {
        let sim = town();
        let view = sim.view();
        let mut tool = LineTool::default();
        assert!(matches!(tool.click(&view, at(4.0, 3.0)), Click::Refused(_)));
        assert!(tool.stops.is_empty());
        assert_eq!(tool.click(&view, at(60.0, 3.0)), Click::Added);
        assert_eq!(tool.click(&view, at(197.0, 100.0)), Click::Added);
        tool.undo();
        assert_eq!(tool.stops.len(), 1);
        tool.cancel();
        assert!(tool.stops.is_empty());
        // Nothing near a road: nothing happens.
        assert_eq!(tool.click(&view, at(100.0, 100.0)), Click::Nothing);
    }

    #[test]
    fn the_ghost_routes_the_ring_in_the_colour_the_line_will_get() {
        let sim = town();
        let view = sim.view();
        let mut tool = LineTool::default();
        assert_eq!(tool.click(&view, at(60.0, 3.0)), Click::Added);
        let g = line_ghost(&view, &tool, at(197.0, 100.0)).unwrap();
        assert_eq!(g.hover.map(|h| h.1), Some(true));
        assert!(
            g.route.len() > 10 && g.length_m > 200.0 && g.cycle_s > 0.0,
            "{g:?}"
        );
        assert_eq!(g.color, next_line_color(&view.transit.net));
        assert!(g.error.is_none());
        // A refused spot is red and carries the rule.
        let bad = line_ghost(&view, &tool, at(4.0, 3.0)).unwrap();
        assert_eq!(bad.hover.map(|h| h.1), Some(false));
        assert!(bad.error.is_some());
    }

    #[test]
    fn a_click_on_a_stop_or_ribbon_picks_the_line_and_edit_sends_edit_line() {
        use cp_sim::Selection;
        let mut sim = town();
        let mut tool = LineTool::default();
        let view = sim.view();
        for p in [at(60.0, 3.0), at(197.0, 100.0), at(100.0, 197.0)] {
            tool.click(&view, p);
        }
        let first = tool.stops[0].pos;
        let Click::Create(SimCommand::Transit(cmd)) = tool.click(&view, first) else {
            panic!("closes");
        };
        cp_transit::apply(sim.world_mut(), &cmd).unwrap();
        sim.tick_once();
        let view = sim.view();
        let line = view.transit.net.lines.iter().next().unwrap().0;
        let stop = view.transit.net.stops.iter().next().unwrap().1.pos;
        // Lines are only picked while the Transit view shows them.
        assert_eq!(
            crate::pick_with(&view, 1.0, stop, 50.0, true),
            Some(Selection::Line(line))
        );
        assert!(!matches!(
            crate::pick_with(&view, 1.0, stop, 50.0, false),
            Some(Selection::Line(_))
        ));
        let mid = view.transit.routes[&line].points[40];
        assert_eq!(
            crate::pick_with(&view, 1.0, mid, 50.0, true),
            Some(Selection::Line(line))
        );
        // Edit: the ring is back in the tool and closing it sends EditLine for that line.
        let mut tool = LineTool::default();
        assert!(tool.begin_edit(&view, line));
        assert_eq!(tool.stops.len(), 3);
        tool.undo();
        assert_eq!(tool.click(&view, at(100.0, 197.0)), Click::Added);
        let first = tool.stops[0].pos;
        let Click::Create(SimCommand::Transit(TransitCommand::EditLine { line: l, stops })) =
            tool.click(&view, first)
        else {
            panic!("an edit sends EditLine");
        };
        assert_eq!((l, stops.len()), (line, 3));
        assert_eq!(tool.editing, None);
    }
}

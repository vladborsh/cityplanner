//! Keeping stops attached to the road network and the plans up to date (`Network` phase).

use bevy_ecs::change_detection::DetectChanges;
use bevy_ecs::system::{Res, ResMut};
use bevy_math::Vec2;
use cp_content::{ContentDb, SimContent};
use cp_core::curve::to_xz;
use cp_net::NetGraph;
use cp_traffic::PathNetwork;

use crate::model::{LineState, StopId, StopKind, TransitNet};
use crate::plan::{LinePlan, LinePlans, plan_line};
use crate::rules::{STATION_SNAP, STOP_SNAP, pavement_offset, pavement_point};

/// Signed lateral distance of `pos` from the axis of `seg` (positive = left) and the arc
/// length of its projection.
fn project(seg: &cp_net::Segment, pos: Vec2) -> (f32, f32) {
    let (t, _) = seg.curve.nearest_xz(pos);
    let d = to_xz(seg.curve.derivative(t)).normalize_or_zero();
    let p = to_xz(seg.curve.point(t));
    let left = Vec2::new(d.y, -d.x);
    let lateral = (pos - p).dot(left);
    (lateral, seg.curve.arc_table().t_to_s(t))
}

/// Re-attaches every road stop to the network after it changed (§16 M3 A): a stop whose
/// segment still carries it stays as it is; one whose segment is gone is projected onto the
/// segment that now lies within [`STOP_SNAP`] of where its pavement was (a cut keeps it); a
/// stop with no such segment is removed, and with it from the lines. Returns the removed
/// stops. Only the position rules of a *new* stop apply — an existing stop is never dropped
/// for standing close to a new junction.
pub fn rebind_stops(net: &NetGraph, db: &ContentDb, transit: &mut TransitNet) -> Vec<StopId> {
    let ids: Vec<StopId> = transit.stops.iter().map(|(id, _)| id).collect();
    let mut gone = Vec::new();
    let mut moves: Vec<(StopId, StopKind, Vec2)> = Vec::new();
    for id in ids {
        let st = transit.stops.get(id).expect("listed");
        // A station is bound to its platform: the tunnel segment under it, which a split
        // may have replaced.
        if let StopKind::Station { building, seg, .. } = st.kind {
            if net.segment(seg).is_some() {
                continue;
            }
            let hit = net
                .segments()
                .filter(|(_, g)| g.elevation == cp_net::Elevation::Tunnel)
                .map(|(sid, g)| (g.curve.nearest_xz(st.pos).1, sid, g))
                .filter(|(d, _, _)| *d <= STATION_SNAP)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            match hit {
                Some((_, sid, g)) => {
                    let (_, s_new) = project(g, st.pos);
                    let kind = StopKind::Station {
                        building,
                        seg: sid,
                        s: s_new,
                    };
                    moves.push((id, kind, st.pos));
                }
                None => gone.push(id),
            }
            continue;
        }
        let StopKind::Road { seg, s } = st.kind else {
            continue;
        };
        let fits = |segment: &cp_net::Segment| -> Option<f32> {
            let off = pavement_offset(db, segment, st.side)?;
            let (lateral, _) = project(segment, st.pos);
            let miss = (lateral - off).abs();
            (miss <= STOP_SNAP).then_some(miss)
        };
        if let Some(segment) = net.segment(seg)
            && fits(segment).is_some()
            && s <= segment.length
        {
            continue;
        }
        // Another segment: the closest fit, the lower id on a tie.
        let mut best: Option<(f32, cp_net::SegmentId)> = None;
        for (cand, segment) in net.segments() {
            let off = pavement_offset(db, segment, st.side).unwrap_or(0.0);
            if segment.curve.bounds_distance_xz(st.pos) > off.abs() + STOP_SNAP {
                continue;
            }
            if let Some(miss) = fits(segment)
                && best.is_none_or(|(m, _)| miss < m)
            {
                best = Some((miss, cand));
            }
        }
        match best {
            Some((_, cand)) => {
                let segment = net.segment(cand).expect("listed");
                let (_, s_new) = project(segment, st.pos);
                let off = pavement_offset(db, segment, st.side).expect("fits");
                let pos = pavement_point(segment, s_new, off);
                moves.push((
                    id,
                    StopKind::Road {
                        seg: cand,
                        s: s_new,
                    },
                    pos,
                ));
            }
            None => gone.push(id),
        }
    }
    for (id, kind, pos) in moves {
        if let Some(st) = transit.stops.get_mut(id) {
            st.kind = kind;
            st.pos = pos;
        }
    }
    for &id in &gone {
        transit.remove_stop(id);
    }
    if !gone.is_empty() {
        transit.relink_stops();
    }
    gone
}

/// Plans `line` on `graph` and sets its state: `Broken` when a leg has no path, `Ok` again
/// when it has (`NoDepot` is T-156's and stays).
pub fn replan_line(
    transit: &mut TransitNet,
    graph: &cp_traffic::PathGraph,
    line: crate::LineId,
) -> LinePlan {
    let Some(l) = transit.lines.get(line) else {
        return LinePlan::default();
    };
    let stops: Vec<_> = l
        .stops
        .iter()
        .filter_map(|&s| transit.access(s).map(|a| (s, a)))
        .collect();
    let plan = match (stops.len() == l.stops.len(), l.mode) {
        (true, crate::TransitMode::Metro) => crate::plan::plan_metro(graph, &stops),
        (true, mode) => plan_line(graph, mode.class(), &stops),
        (false, mode) => plan_line(graph, mode.class(), &[]),
    };
    if let Some(l) = transit.lines.get_mut(line) {
        l.state = match (plan.is_ok(), l.state) {
            (false, _) => LineState::Broken,
            (true, LineState::Broken) => LineState::Ok,
            (true, other) => other,
        };
    }
    plan
}

/// `Network` phase, after `update_path_network`: re-attach the stops when the network
/// changed (and once after a load), then re-plan every line when the stops changed or the
/// `PathGraph` was rebuilt. Does nothing without content or a built graph.
pub fn update_transit(
    net: Res<NetGraph>,
    content: Option<Res<SimContent>>,
    pn: Res<PathNetwork>,
    mut transit: ResMut<TransitNet>,
    mut plans: ResMut<LinePlans>,
) {
    let Some(content) = content else {
        return;
    };
    if net.is_changed() || !plans.bound {
        plans.bound = true;
        if transit.stops.iter().next().is_some() {
            let gone = rebind_stops(&net, &content.0, &mut transit);
            if !gone.is_empty() {
                plans.dirty = true;
            }
        }
    }
    if pn.generation() == 0 {
        return;
    }
    if plans.dirty || plans.generation != pn.generation() {
        let ids: Vec<_> = transit.lines.iter().map(|(id, _)| id).collect();
        plans.by_line.clear();
        for id in ids {
            let plan = replan_line(&mut transit, pn.graph(), id);
            plans.by_line.insert(id, plan);
        }
        plans.generation = pn.generation();
        plans.dirty = false;
    }
}

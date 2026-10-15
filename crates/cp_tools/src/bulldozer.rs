//! The segment bulldozer: finds the segment under the cursor.

use bevy_math::Vec2;
use cp_net::{
    NetCommand, NetGraph, PART_SNAP, SegmentId, curve_outside_owned, locate, road_run, span, stops,
    stretch,
};
use cp_sim::SimCommand;
use cp_terrain::TileOwnership;

/// Pick radius around the cursor, metres.
pub const PICK_RADIUS: f32 = 8.0;

/// The segment nearest to `p` within `radius` (XZ distance to its centre line; ties go to the
/// lower id). Control-point bounds reject most segments before the exact distance is taken.
pub fn pick_segment(net: &NetGraph, p: Vec2, radius: f32) -> Option<SegmentId> {
    let mut best: Option<(f32, SegmentId)> = None;
    for (id, seg) in net.segments() {
        let pts = seg.curve.xz();
        let (lo, hi) = pts
            .iter()
            .fold((pts[0], pts[0]), |(lo, hi), q| (lo.min(*q), hi.max(*q)));
        if p.x < lo.x - radius || p.x > hi.x + radius || p.y < lo.y - radius || p.y > hi.y + radius
        {
            continue;
        }
        let (_, d) = seg.curve.nearest_xz(p);
        if d <= radius && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, id));
        }
    }
    best.map(|(_, id)| id)
}

/// A piece of a segment the bulldozer takes: `(segment, s0, s1)`, arc lengths from the
/// segment's start.
pub type Span = (SegmentId, f32, f32);

/// What a click at `p` takes (`architecture/system-design.md` §6.7): the part of the stretch
/// under the cursor between the nearest stops on either side (a junction, a corner, a dead end,
/// a change of road type or height, or a control point), or with `whole_run` the whole road
/// run (Shift). Ends within [`PART_SNAP`] of a segment end are moved onto it, exactly as
/// `RemovePart` does, so what is highlighted is what goes.
pub fn demolition(net: &NetGraph, p: Vec2, radius: f32, whole_run: bool) -> Vec<Span> {
    let Some(seg) = pick_segment(net, p, radius) else {
        return Vec::new();
    };
    if whole_run {
        return road_run(net, seg)
            .into_iter()
            .filter_map(|id| net.segment(id).map(|s| (id, 0.0, s.length)))
            .collect();
    }
    let Some(st) = stretch(net, seg) else {
        return Vec::new();
    };
    let Some(s) = net.segment(seg) else {
        return Vec::new();
    };
    let (t, _) = s.curve.nearest_xz(p);
    let Some(along) = locate(net, seg, s.curve.arc_table().t_to_s(t)) else {
        return Vec::new();
    };
    let stops = stops(net, &st);
    let Some(last) = stops.len().checked_sub(1) else {
        return Vec::new();
    };
    // The first stop beyond the cursor closes the piece, the one before it opens it.
    let hi = stops
        .iter()
        .position(|q| q.along > along + 1e-3)
        .unwrap_or(last)
        .max(1);
    span(net, &st, stops[hi - 1].along, stops[hi].along)
        .into_iter()
        .filter_map(|(id, s0, s1)| {
            let len = net.segment(id)?.length;
            let s0 = if s0 < PART_SNAP { 0.0 } else { s0 };
            let s1 = if s1 > len - PART_SNAP { len } else { s1 };
            (s1 - s0 >= cp_net::MIN_SEGMENT_LENGTH).then_some((id, s0, s1))
        })
        .collect()
}

/// `RemoveSegment` for a piece that is a whole segment, `RemovePart` otherwise; one command per
/// span, in order. The tool pushes them all in one frame, so they run in one tick.
pub fn span_commands(net: &NetGraph, spans: &[Span]) -> Vec<SimCommand> {
    spans
        .iter()
        .map(|&(seg, s0, s1)| {
            let whole = net
                .segment(seg)
                .is_some_and(|s| s0 <= 0.0 && s1 >= s.length);
            SimCommand::Net(if whole {
                NetCommand::RemoveSegment { seg }
            } else {
                NetCommand::RemovePart { seg, s0, s1 }
            })
        })
        .collect()
}

/// Whether any segment touched leaves the bought tiles (`RemovePart` checks the whole segment).
pub fn spans_outside_owned(net: &NetGraph, tiles: &TileOwnership, spans: &[Span]) -> bool {
    spans.iter().any(|&(id, ..)| {
        net.segment(id)
            .is_some_and(|s| curve_outside_owned(tiles, &s.curve).is_some())
    })
}

/// Total length of the spans, metres.
pub fn spans_length(spans: &[Span]) -> f32 {
    spans.iter().map(|&(_, a, b)| b - a).sum()
}

/// One `RemoveSegment` per segment of a road run, in run order. The tool pushes them all in
/// one frame, so the whole run goes in a single tick (no new command, no save change).
pub fn remove_run_commands(run: &[SegmentId]) -> Vec<SimCommand> {
    run.iter()
        .map(|&seg| SimCommand::Net(NetCommand::RemoveSegment { seg }))
        .collect()
}

/// Whether any segment of the run leaves the bought tiles: the bulldozer ghost is then
/// invalid ("outside your land") and no command is sent (`RemoveSegment` would be `NotOwned`).
pub fn run_outside_owned(net: &NetGraph, tiles: &TileOwnership, run: &[SegmentId]) -> bool {
    run.iter().any(|&id| {
        net.segment(id)
            .is_some_and(|s| curve_outside_owned(tiles, &s.curve).is_some())
    })
}

/// The segment's centre line as XZ points (for highlighting).
pub fn centre_line(net: &NetGraph, id: SegmentId, step: f32) -> Vec<bevy_math::Vec3> {
    net.segment(id)
        .map(|s| crate::road::sample_curve(&s.curve, step))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec3;
    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;
    use cp_net::Elevation;

    fn all_owned() -> TileOwnership {
        let mut tiles = TileOwnership::none();
        for tz in 0..cp_terrain::TILES_PER_SIDE {
            for tx in 0..cp_terrain::TILES_PER_SIDE {
                tiles.set(tx, tz, true);
            }
        }
        tiles
    }

    fn net_with_two_roads() -> (NetGraph, SegmentId, SegmentId) {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        let mut net = NetGraph::new();
        let a = net
            .add_segment(
                &def,
                road,
                CubicBezier3::line(
                    Vec3::new(1000.0, 0.0, 1000.0),
                    Vec3::new(1100.0, 0.0, 1000.0),
                ),
                Elevation::Ground,
            )
            .unwrap();
        let b = net
            .add_segment(
                &def,
                road,
                CubicBezier3::line(
                    Vec3::new(1000.0, 0.0, 1050.0),
                    Vec3::new(1100.0, 0.0, 1050.0),
                ),
                Elevation::Ground,
            )
            .unwrap();
        (net, a, b)
    }

    #[test]
    fn picks_the_nearest_segment_within_the_radius() {
        let (net, a, b) = net_with_two_roads();
        assert_eq!(
            pick_segment(&net, Vec2::new(1050.0, 1003.0), PICK_RADIUS),
            Some(a)
        );
        assert_eq!(
            pick_segment(&net, Vec2::new(1050.0, 1046.0), PICK_RADIUS),
            Some(b)
        );
        // Between them, nearer to b.
        assert_eq!(pick_segment(&net, Vec2::new(1050.0, 1030.0), 30.0), Some(b));
        // Too far from both.
        assert_eq!(
            pick_segment(&net, Vec2::new(1050.0, 1025.0), PICK_RADIUS),
            None
        );
        assert_eq!(
            pick_segment(&net, Vec2::new(2000.0, 2000.0), PICK_RADIUS),
            None
        );
        assert_eq!(
            pick_segment(&NetGraph::new(), Vec2::ZERO, PICK_RADIUS),
            None
        );
    }

    #[test]
    fn centre_line_follows_the_segment() {
        let (net, a, _) = net_with_two_roads();
        let line = centre_line(&net, a, 10.0);
        assert!(line.len() >= 10);
        assert!(
            (line[0].x - 1000.0).abs() < 1e-3 && (line.last().unwrap().x - 1100.0).abs() < 1e-3
        );
    }

    #[test]
    fn a_click_on_the_middle_of_a_long_street_removes_it_up_to_the_crossings() {
        use crate::CommandOutbox;
        use cp_net::road_run;
        use cp_sim::SimRunner;

        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        let mut sim = SimRunner::new(1);
        sim.set_content(std::sync::Arc::new(db));
        sim.world_mut().insert_resource(all_owned());
        let line = |a: (f32, f32), b: (f32, f32)| {
            CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1))
        };
        {
            let mut net = sim.world_mut().resource_mut::<NetGraph>();
            for i in 0..4 {
                let x = 1000.0 + 75.0 * i as f32;
                net.add_segment(
                    &def,
                    road,
                    line((x, 1000.0), (x + 75.0, 1000.0)),
                    Elevation::Ground,
                )
                .unwrap();
            }
            // Crossing arms at both ends.
            for (a, b) in [
                ((920.0, 1000.0), (1000.0, 1000.0)),
                ((1300.0, 1000.0), (1380.0, 1000.0)),
                ((1000.0, 920.0), (1000.0, 1000.0)),
                ((1300.0, 1000.0), (1300.0, 1080.0)),
            ] {
                net.add_segment(&def, road, line(a, b), Elevation::Ground)
                    .unwrap();
            }
        }
        sim.tick_once();
        let view = sim.view();
        let seg = pick_segment(&view.net, Vec2::new(1150.0, 1002.0), PICK_RADIUS).unwrap();
        let run = road_run(&view.net, seg);
        assert_eq!(run.len(), 4);

        let mut outbox = CommandOutbox::default();
        for c in remove_run_commands(&run) {
            outbox.push(c);
        }
        outbox.flush_into(&mut sim);
        sim.tick_once();
        let after = sim.view();
        assert_eq!(after.net.segment_count(), 4, "only the four arms are left");
        assert!(run.iter().all(|s| after.net.segment(*s).is_none()));
    }

    #[test]
    fn a_bulldozer_run_half_on_an_unbought_tile_is_invalid() {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let road = db.road_id("basic_2l").unwrap();
        let t = cp_terrain::TILE_SIZE;
        let mut net = NetGraph::new();
        let seg = |net: &mut NetGraph, x0: f32, x1: f32| {
            net.add_segment(
                db.road(road),
                road,
                CubicBezier3::line(Vec3::new(x0, 0.0, 1000.0), Vec3::new(x1, 0.0, 1000.0)),
                Elevation::Ground,
            )
            .unwrap()
        };
        let straddling = seg(&mut net, t - 50.0, t + 50.0);
        let inside = seg(&mut net, 100.0, 200.0);
        // Only the tile column of x = 0 is bought.
        let mut tiles = TileOwnership::none();
        let (tx, tz) = cp_terrain::Terrain::tile_of(Vec2::new(100.0, 1000.0));
        tiles.set(tx, tz, true);
        assert!(run_outside_owned(&net, &tiles, &[straddling]));
        assert!(run_outside_owned(&net, &tiles, &[inside, straddling]));
        assert!(!run_outside_owned(&net, &tiles, &[inside]));
    }
}

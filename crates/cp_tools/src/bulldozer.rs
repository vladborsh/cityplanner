//! The segment bulldozer: finds the segment under the cursor.

use bevy_math::Vec2;
use cp_net::{NetGraph, SegmentId};

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
}

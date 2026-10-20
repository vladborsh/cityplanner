//! The buildings a road would stand on (BUG-096): a road laid or widened over a lot takes the
//! building down. A grown one ([`crate::is_growable`], as for a service placed over houses,
//! T-252) goes without asking; a protected one only with the player's consent. One pure
//! function, shared by the command layer (which demolishes them, or refuses) and the tools'
//! ghosts (which tint them red), so the ghost shows what the click does.

use bevy_math::Vec2;
use cp_core::curve::{CubicBezier3, to_xz};

use crate::grid::BuildingGrid;
use crate::model::BuildingId;

/// How far a footprint may reach into a road's strip and still stand (metres): the zoning's own
/// tolerance, so a building on the cells beside a road is never hit.
const TOUCH_M: f32 = cp_zone::grid::ROAD_EPS + 0.02;

/// Pieces a strip's curve is followed in.
const PIECES: usize = 32;

/// The buildings whose footprint overlaps one of `strips` (a road's curve with its half width:
/// asphalt and sidewalks), in id order.
pub fn buildings_under_road(
    buildings: &BuildingGrid,
    strips: &[(CubicBezier3, f32)],
) -> Vec<BuildingId> {
    if strips.is_empty() {
        return Vec::new();
    }
    let lines: Vec<(Vec<Vec2>, f32, &CubicBezier3)> = strips
        .iter()
        .map(|(c, hw)| {
            let pts = (0..=PIECES)
                .map(|i| to_xz(c.point(i as f32 / PIECES as f32)))
                .collect();
            (pts, *hw, c)
        })
        .collect();
    let mut out: Vec<BuildingId> = buildings
        .iter()
        .filter(|(_, b)| {
            let rect = crate::spawn::rect_of(b.pos, b.rot, b.size());
            let reach = rect.half.hypot(rect.half_v);
            lines.iter().any(|(pts, hw, c)| {
                c.bounds_distance_xz(b.pos) <= hw + reach
                    && pts
                        .windows(2)
                        .any(|w| rect.distance_to_segment(w[0], w[1]) < hw - TOUCH_M)
            })
        })
        .map(|(id, _)| id)
        .collect();
    out.sort();
    out
}

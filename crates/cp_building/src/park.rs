//! The wellbeing parks give (T-237): the households near a park fall ill less and are happier
//! (the leisure factor of contentment, T-318). Derived from the active park buildings each time
//! it is needed; nothing is saved.

use bevy_math::Vec2;
use cp_content::ContentDb;

use crate::grid::BuildingGrid;
use crate::model::BuildingState;

/// Sickness of the residents of a home with full wellbeing is cut by this share. // tune later
pub const GREEN_SICK_CUT: f32 = 0.6;

/// One park: where it stands and how far its wellbeing reaches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParkSite {
    pub pos: Vec2,
    pub radius_m: f32,
    pub green: f32,
}

/// The active buildings whose definition gives wellbeing, in building order.
pub fn park_sites(db: &ContentDb, buildings: &BuildingGrid) -> Vec<ParkSite> {
    buildings
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .filter_map(|(_, b)| {
            let e = db.building(b.def).land_effect?;
            (e.green > 0.0 && e.radius_m > 0.0).then_some(ParkSite {
                pos: b.pos,
                radius_m: e.radius_m,
                green: e.green,
            })
        })
        .collect()
}

/// The wellbeing at `pos`, `0..=1`: the parks' shares added, each falling off linearly with
/// the distance.
pub fn green_at(parks: &[ParkSite], pos: Vec2) -> f32 {
    parks
        .iter()
        .map(|p| p.green * (1.0 - pos.distance(p.pos) / p.radius_m).max(0.0))
        .sum::<f32>()
        .min(1.0)
}

/// The chance per day of falling ill, from the base rate, the pollution part and the wellbeing.
pub fn sick_chance(base: f32, pollution_part: f32, green: f32) -> f32 {
    (base + pollution_part) * (1.0 - GREEN_SICK_CUT * green.clamp(0.0, 1.0))
}

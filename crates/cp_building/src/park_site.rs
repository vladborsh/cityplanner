//! Where the buildings of the park areas may stand (T-223, `architecture/system-design.md` §16
//! M6 (C)).
//!
//! [`validate_park_site`] is the pure check the ghost previews with and `Place` / `Move` commit
//! with, run on a [`PlacementPlan`] that the geometry checks already accepted:
//!
//! * a **gate** (on a road) must touch a cell of an area of its own kind;
//! * an **attraction** of a kind has its lot centre in an area of that kind, and the area must
//!   have reached `min_level`; a city park (no kind) has no area rule;
//! * a **prop** stands in any park area.

use bevy_math::Vec2;
use cp_content::{BuildingDefId, ContentDb, ParkKind, ParkRole};
use cp_district::{AreaKind, AreaMap, Areas, area_at};
use cp_zone::ZoneGrid;

use crate::command::{BuildingError, PlacementPlan};
use crate::industry::plan_points;
use crate::model::FOOTPRINT_CELL_M;

/// The inputs of the rule, borrowed together.
#[derive(Clone, Copy)]
pub struct ParkWorld<'a> {
    pub db: &'a ContentDb,
    pub areas: &'a Areas,
    pub area_map: &'a AreaMap,
    pub zones: &'a ZoneGrid,
}

/// The park area (kind and level) at a point.
fn park_at(w: &ParkWorld, p: Vec2) -> Option<(ParkKind, u8)> {
    let a = w.areas.get(area_at(w.area_map, p)?)?;
    match a.kind {
        AreaKind::Park(k) => Some((k, a.level)),
        AreaKind::Industry(_) => None,
    }
}

/// The pure placement rule for the park buildings. Defs without `park` always pass.
pub fn validate_park_site(
    w: &ParkWorld,
    def: BuildingDefId,
    plan: &PlacementPlan,
) -> Result<(), BuildingError> {
    let Some(spec) = &w.db.building(def).park else {
        return Ok(());
    };
    match (spec.role, spec.park) {
        (ParkRole::Gate, Some(kind)) => {
            // A cell of the lot, or one next to it, lies in an area of the kind.
            let step = FOOTPRINT_CELL_M;
            let touches = plan_points(w.zones, plan).into_iter().any(|c| {
                [
                    Vec2::ZERO,
                    Vec2::new(step, 0.0),
                    Vec2::new(-step, 0.0),
                    Vec2::new(0.0, step),
                    Vec2::new(0.0, -step),
                ]
                .iter()
                .any(|&d| park_at(w, c + d).is_some_and(|(k, _)| k == kind))
            });
            if touches {
                Ok(())
            } else {
                Err(BuildingError::GateAway(kind))
            }
        }
        (ParkRole::Attraction, Some(kind)) => match park_at(w, plan.pos) {
            Some((k, level)) if k == kind => {
                if level < spec.min_level {
                    Err(BuildingError::AreaLevel {
                        need: spec.min_level,
                        have: level,
                    })
                } else {
                    Ok(())
                }
            }
            _ => Err(BuildingError::NotInPark(Some(kind))),
        },
        (ParkRole::Prop, _) => park_at(w, plan.pos)
            .map(|_| ())
            .ok_or(BuildingError::NotInPark(None)),
        // A city park, or a malformed gate: no area rule.
        _ => Ok(()),
    }
}

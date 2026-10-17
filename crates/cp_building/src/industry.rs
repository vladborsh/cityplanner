//! Where the buildings of the industrial chains may stand (T-208, `architecture/system-design.md`
//! §16 M5 (B) «Размещение»).
//!
//! [`validate_industry_site`] is the pure check the ghost previews with and `Place` / `Move`
//! commit with, run on a [`PlacementPlan`] that the geometry checks already accepted. The centre
//! of the lot must be in an industrial area of the building's resource; a Main building is one
//! per area; every other building needs a Main in the area and `area.level >= min_level`; an
//! Extractor needs [`lot_richness`] of at least `Balance::min_richness`.

use bevy_math::Vec2;
use cp_content::{BuildingDefId, ContentDb, IndustryRole, ResourceKind};
use cp_district::{AreaKind, AreaMap, Areas, area_at};
use cp_terrain::ResourceMap;
use cp_zone::ZoneGrid;

use crate::command::{BuildingError, PlacementPlan};
use crate::grid::BuildingGrid;
use crate::model::{BuildingId, Footprint, footprint_quad};

/// The centres of the 8 m cells a plan covers (lot cells, or the lattice of a free rectangle).
pub fn plan_points(zones: &ZoneGrid, plan: &PlacementPlan) -> Vec<Vec2> {
    match &plan.footprint {
        Footprint::Lot(lot) => lot
            .cells()
            .filter_map(|c| zones.cell_footprint(c).map(|o| o.c))
            .collect(),
        Footprint::Free { w, d } => {
            let q = footprint_quad(plan.pos, plan.rot, *w, *d);
            let (ax, ay) = (q[1] - q[0], q[3] - q[0]);
            let (nu, nv) = (usize::from(*w), usize::from(*d));
            (0..nv)
                .flat_map(|j| (0..nu).map(move |i| (i, j)))
                .map(|(i, j)| {
                    q[0] + ax * ((i as f32 + 0.5) / nu as f32) + ay * ((j as f32 + 0.5) / nv as f32)
                })
                .collect()
        }
    }
}

/// Average richness (`0..=1`) of `kind` under the given cell centres of a lot. Shared by the
/// placement rule and the ghost, which shows it as a percentage.
pub fn lot_richness(resources: &ResourceMap, kind: ResourceKind, points: &[Vec2]) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    points
        .iter()
        .map(|&p| resources.richness_at(kind, p))
        .sum::<f32>()
        / points.len() as f32
}

/// The industrial area (id and record) the point is in, if it is one.
fn industry_area<'a>(
    areas: &'a Areas,
    map: &AreaMap,
    p: Vec2,
) -> Option<(cp_district::AreaId, &'a cp_district::Area)> {
    let id = area_at(map, p)?;
    let a = areas.get(id)?;
    matches!(a.kind, AreaKind::Industry(_)).then_some((id, a))
}

/// The inputs of the rule, borrowed together.
#[derive(Clone, Copy)]
pub struct IndustryWorld<'a> {
    pub db: &'a ContentDb,
    pub areas: &'a Areas,
    pub area_map: &'a AreaMap,
    pub resources: &'a ResourceMap,
    pub buildings: &'a BuildingGrid,
    pub zones: &'a ZoneGrid,
}

/// The pure placement rule for the chain buildings. Defs without `industry` always pass.
/// `ignore` is the building being moved (not an obstacle to itself).
pub fn validate_industry_site(
    w: &IndustryWorld,
    def: BuildingDefId,
    plan: &PlacementPlan,
    ignore: Option<BuildingId>,
) -> Result<(), BuildingError> {
    let Some(spec) = &w.db.building(def).industry else {
        return Ok(());
    };
    let (area_id, area) = industry_area(w.areas, w.area_map, plan.pos)
        .ok_or(BuildingError::NotInArea(spec.resource))?;
    let AreaKind::Industry(area_res) = area.kind else {
        unreachable!("industry_area returns industrial areas");
    };
    if spec.resource.is_some_and(|r| r != area_res) {
        return Err(BuildingError::NotInArea(spec.resource));
    }
    let has_main = || {
        w.buildings.iter().any(|(id, b)| {
            Some(id) != ignore
                && w.db
                    .building(b.def)
                    .industry
                    .as_ref()
                    .is_some_and(|s| s.role == IndustryRole::Main)
                && area_at(w.area_map, b.pos) == Some(area_id)
        })
    };
    if spec.role == IndustryRole::Main {
        if has_main() {
            return Err(BuildingError::SecondMain);
        }
        return Ok(());
    }
    if !has_main() {
        return Err(BuildingError::NoMain);
    }
    if area.level < spec.min_level {
        return Err(BuildingError::AreaLevel {
            need: spec.min_level,
            have: area.level,
        });
    }
    if spec.role == IndustryRole::Extractor
        && let Some(res) = spec.resource
    {
        let have = lot_richness(w.resources, res, &plan_points(w.zones, plan));
        let need = w.db.balance().min_richness;
        if have < need {
            return Err(BuildingError::Poor {
                pct: (have * 100.0).round() as u8,
                need_pct: (need * 100.0).round() as u8,
            });
        }
    }
    Ok(())
}

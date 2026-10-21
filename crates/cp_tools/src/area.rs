//! The area brush (T-207): paints or clears 32 m cells of the area map, like the district
//! brush (T-133) and with the same brush kinds. A stroke sends one `PaintArea` / `EraseArea`
//! per step for the cells that change.

use std::collections::BTreeSet;

use bevy_math::Vec2;
use cp_content::{ParkKind, ResourceKind};
use cp_district::{AreaId, AreaKind, AreaMap, Areas, Cell, DistrictCommand, MAX_AREAS};
use cp_locale::{tr, trf};

use crate::district::DistrictToolKind;

/// The id the next `CreateArea` will get: the lowest free slot (the rule of `cp_district`).
pub fn next_area(areas: &Areas) -> Option<AreaId> {
    let list = &areas.list;
    let slot = list.iter().position(Option::is_none).unwrap_or(list.len());
    (slot < MAX_AREAS).then(|| AreaId(slot as u8 + 1))
}

/// The name of a park kind as the player reads it.
pub fn park_kind_name(kind: ParkKind) -> &'static str {
    match kind {
        ParkKind::CityPark => "City park",
        ParkKind::Nature => "Nature reserve",
        ParkKind::Zoo => "Zoo",
        ParkKind::Amusement => "Amusement park",
    }
}

/// "Forestry area 3" or "Zoo 3", with `3` the id the new area gets.
pub fn new_area_name(kind: AreaKind, id: AreaId) -> String {
    let n = id.0;
    match kind {
        AreaKind::Industry(ResourceKind::Farming) => trf!("Farming area {n}", n = n),
        AreaKind::Industry(ResourceKind::Forestry) => trf!("Forestry area {n}", n = n),
        AreaKind::Industry(ResourceKind::Ore) => trf!("Ore area {n}", n = n),
        AreaKind::Industry(ResourceKind::Oil) => trf!("Oil area {n}", n = n),
        AreaKind::Park(k) => format!("{} {}", tr(park_kind_name(k)), n),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AreaTool {
    pub kind: DistrictToolKind,
    /// The area the brushes paint (the one chosen in the list).
    pub selected: Option<AreaId>,
    /// The resource "New area" makes an area of.
    pub new_resource: Option<ResourceKind>,
    /// The park kind "New area" of the Parks category makes an area of (T-226).
    pub new_park: Option<ParkKind>,
    /// Cells already visited in the current stroke.
    pub stroke: BTreeSet<Cell>,
}

impl AreaTool {
    pub fn set_kind(&mut self, kind: DistrictToolKind) {
        self.kind = kind;
        self.stroke.clear();
    }

    pub fn cancel(&mut self) {
        self.stroke.clear();
    }

    /// The kind of area a stroke paints: the chosen area's, else the park kind or the resource
    /// picked last in the panel, else the richest resource under the cursor (`richness`), else
    /// farming. The ghost shows its colour (BUG-108).
    pub fn brushed_kind(&self, areas: &Areas, richness: impl Fn(ResourceKind) -> f32) -> AreaKind {
        if let Some(a) = self.selected.and_then(|id| areas.get(id)) {
            return a.kind;
        }
        match (self.new_park, self.new_resource) {
            (Some(k), _) => AreaKind::Park(k),
            (None, Some(r)) => AreaKind::Industry(r),
            (None, None) => {
                let mut best = (ResourceKind::Farming, 0.0);
                for r in ResourceKind::ALL {
                    let v = richness(r);
                    if v > best.1 {
                        best = (r, v);
                    }
                }
                AreaKind::Industry(best.0)
            }
        }
    }

    /// A brush stroke at `at` with no area chosen (BUG-108): an area of the kind being brushed
    /// is chosen, else made. The kind is the park kind or the resource last picked in the panel,
    /// else the richest resource under the cursor (`richness`), else farming. Returns the
    /// `CreateArea` to send before the stroke's paint. The eraser needs none.
    pub fn choose_or_create(
        &mut self,
        areas: &Areas,
        richness: impl Fn(ResourceKind) -> f32,
    ) -> Option<DistrictCommand> {
        if self.selected.is_some() || self.kind == DistrictToolKind::Erase {
            return None;
        }
        let kind = self.brushed_kind(areas, richness);
        if let Some((id, _)) = areas.iter().find(|(_, a)| a.kind == kind) {
            self.selected = Some(id);
            return None;
        }
        let id = next_area(areas)?;
        self.selected = Some(id);
        Some(DistrictCommand::CreateArea {
            kind,
            name: new_area_name(kind, id),
        })
    }

    /// The button was released: the next press starts a new stroke.
    pub fn end_stroke(&mut self) {
        self.stroke.clear();
    }

    /// The cells under the brush with the cursor at `hover` (the highlight).
    pub fn preview(&self, hover: Vec2) -> Vec<Cell> {
        crate::district::brush_cells(hover, self.kind)
    }

    /// One stroke step over `cells`: the one `PaintArea` / `EraseArea` for the cells that
    /// change, or `None` when nothing does. Every cell passed is remembered for the stroke.
    pub fn stroke_cells(&mut self, map: &AreaMap, cells: &[Cell]) -> Option<DistrictCommand> {
        let changing: Vec<Cell> = cells
            .iter()
            .copied()
            .filter(|c| !self.stroke.contains(c))
            .filter(|&c| match self.kind {
                DistrictToolKind::Erase => map.get(c).is_some(),
                _ => self.selected.is_some() && map.get(c) != self.selected,
            })
            .collect();
        self.stroke.extend(cells.iter().copied());
        if changing.is_empty() {
            return None;
        }
        Some(match (self.kind, self.selected) {
            (DistrictToolKind::Erase, _) | (_, None) => {
                DistrictCommand::EraseArea { cells: changing }
            }
            (_, Some(area)) => DistrictCommand::PaintArea {
                cells: changing,
                area,
            },
        })
    }

    /// The cursor at `p` with the button held (also the press itself).
    pub fn drag(&mut self, map: &AreaMap, p: Vec2) -> Option<DistrictCommand> {
        let cells = self.preview(p);
        self.stroke_cells(map, &cells)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(kind: DistrictToolKind) -> AreaTool {
        AreaTool {
            kind,
            selected: Some(AreaId(1)),
            ..Default::default()
        }
    }

    #[test]
    fn a_stroke_over_three_cells_sends_one_paint_area_and_the_eraser_only_hits_painted_ones() {
        let mut map = AreaMap::default();
        let mut t = tool(DistrictToolKind::BrushSmall);
        let cells = [(5, 5), (6, 5), (7, 5)];
        assert_eq!(
            t.stroke_cells(&map, &cells),
            Some(DistrictCommand::PaintArea {
                cells: cells.to_vec(),
                area: AreaId(1)
            })
        );
        assert_eq!(
            t.stroke_cells(&map, &cells),
            None,
            "same stroke, nothing new"
        );
        let mut e = tool(DistrictToolKind::Erase);
        assert_eq!(e.stroke_cells(&map, &cells), None, "nothing to clear");
        map.set((6, 5), Some(AreaId(2)));
        e.end_stroke();
        assert_eq!(
            e.stroke_cells(&map, &cells),
            Some(DistrictCommand::EraseArea {
                cells: vec![(6, 5)]
            })
        );
        // No area chosen: a brush paints nothing.
        let mut none = AreaTool::default();
        assert_eq!(none.stroke_cells(&AreaMap::default(), &cells), None);
    }
}

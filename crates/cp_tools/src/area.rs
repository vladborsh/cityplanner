//! The area brush (T-207): paints or clears 32 m cells of the area map, like the district
//! brush (T-133) and with the same brush kinds. A stroke sends one `PaintArea` / `EraseArea`
//! per step for the cells that change.

use std::collections::BTreeSet;

use bevy_math::Vec2;
use cp_content::{ParkKind, ResourceKind};
use cp_district::{AreaId, AreaMap, Cell, DistrictCommand};

use crate::district::{BRUSH_LARGE_RADIUS, DistrictToolKind, cells_within};

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

    /// The button was released: the next press starts a new stroke.
    pub fn end_stroke(&mut self) {
        self.stroke.clear();
    }

    /// The cells under the brush with the cursor at `hover` (the highlight).
    pub fn preview(&self, hover: Vec2) -> Vec<Cell> {
        let radius = match self.kind {
            DistrictToolKind::BrushLarge => BRUSH_LARGE_RADIUS,
            _ => 0.0,
        };
        cells_within(hover, radius)
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

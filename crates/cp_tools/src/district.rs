//! The district brush (T-133): paints or clears 32 m cells of the district map.
//!
//! Pure like the zone tool: given the map and a cursor position it says which cells a stroke
//! step would change, and returns the one [`DistrictCommand`] for them; the plugin pushes it.

use std::collections::BTreeSet;

use bevy_math::Vec2;
use cp_district::{CELL_M, Cell, DistrictCommand, DistrictId, DistrictMap};

/// Cells within this many metres of the cursor (centre to centre) are in the big brush.
pub const BRUSH_LARGE_RADIUS: f32 = 3.0 * CELL_M;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DistrictToolKind {
    /// The cell under the cursor.
    #[default]
    BrushSmall,
    /// Cells within three cells of the cursor.
    BrushLarge,
    /// Clears the cells under the small brush.
    Erase,
}

impl DistrictToolKind {
    pub const ALL: [DistrictToolKind; 3] = [Self::BrushSmall, Self::BrushLarge, Self::Erase];

    pub fn label(self) -> &'static str {
        match self {
            Self::BrushSmall => "Brush S",
            Self::BrushLarge => "Brush L",
            Self::Erase => "Erase",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DistrictTool {
    pub kind: DistrictToolKind,
    /// The district the brushes paint (the one chosen in the list).
    pub selected: Option<DistrictId>,
    /// Cells already visited in the current stroke.
    pub stroke: BTreeSet<Cell>,
}

/// Cells whose centre is within `radius` of `p`; the cell under `p` is always in.
pub fn cells_within(p: Vec2, radius: f32) -> Vec<Cell> {
    let Some((cx, cz)) = DistrictMap::cell_of(p) else {
        return Vec::new();
    };
    let reach = (radius / CELL_M).ceil() as i32 + 1;
    let mut out = Vec::new();
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            let (x, z) = (i32::from(cx) + dx, i32::from(cz) + dz);
            if x < 0 || z < 0 {
                continue;
            }
            let cell = (x as u16, z as u16);
            if DistrictMap::cell_of(DistrictMap::cell_center(cell)).is_none() {
                continue;
            }
            if (dx, dz) == (0, 0) || DistrictMap::cell_center(cell).distance(p) <= radius {
                out.push(cell);
            }
        }
    }
    out
}

impl DistrictTool {
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

    /// The cells of `cells` this tool would change, minus those the stroke already visited.
    fn changing(&self, map: &DistrictMap, cells: &[Cell]) -> Vec<Cell> {
        cells
            .iter()
            .copied()
            .filter(|c| !self.stroke.contains(c))
            .filter(|&c| match self.kind {
                DistrictToolKind::Erase => map.get(c).is_some(),
                _ => self.selected.is_some() && map.get(c) != self.selected,
            })
            .collect()
    }

    /// One stroke step over `cells`: the one `Paint` / `Erase` for the cells that change, or
    /// `None` when nothing does (no district chosen, or already painted). Every cell passed is
    /// remembered for the rest of the stroke.
    pub fn stroke_cells(&mut self, map: &DistrictMap, cells: &[Cell]) -> Option<DistrictCommand> {
        let changing = self.changing(map, cells);
        self.stroke.extend(cells.iter().copied());
        if changing.is_empty() {
            return None;
        }
        Some(match (self.kind, self.selected) {
            (DistrictToolKind::Erase, _) | (_, None) => DistrictCommand::Erase { cells: changing },
            (_, Some(district)) => DistrictCommand::Paint {
                cells: changing,
                district,
            },
        })
    }

    /// The cursor at `p` with the button held (also the press itself).
    pub fn drag(&mut self, map: &DistrictMap, p: Vec2) -> Option<DistrictCommand> {
        let cells = self.preview(p);
        self.stroke_cells(map, &cells)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(kind: DistrictToolKind) -> DistrictTool {
        DistrictTool {
            kind,
            selected: Some(DistrictId(1)),
            stroke: BTreeSet::new(),
        }
    }

    #[test]
    fn a_stroke_over_three_cells_sends_one_paint_with_those_cells() {
        let map = DistrictMap::default();
        let mut t = tool(DistrictToolKind::BrushSmall);
        let cells = [(5, 5), (6, 5), (7, 5)];
        assert_eq!(
            t.stroke_cells(&map, &cells),
            Some(DistrictCommand::Paint {
                cells: cells.to_vec(),
                district: DistrictId(1)
            })
        );
        // The same cells again in the same stroke: nothing more to send.
        assert_eq!(t.stroke_cells(&map, &cells), None);
    }

    #[test]
    fn brushes_cover_the_expected_cells_and_the_eraser_only_hits_painted_ones() {
        let mut map = DistrictMap::default();
        let p = Vec2::new(16.0 * CELL_M + 5.0, 16.0 * CELL_M + 5.0);
        assert_eq!(
            tool(DistrictToolKind::BrushSmall).preview(p),
            vec![(16, 16)]
        );
        let big = tool(DistrictToolKind::BrushLarge).preview(p);
        assert!(big.contains(&(16, 16)) && big.contains(&(18, 16)) && big.len() > 20);
        assert!(!big.contains(&(20, 16)) && !big.contains(&(19, 19)));
        let mut e = tool(DistrictToolKind::Erase);
        assert_eq!(e.drag(&map, p), None, "nothing to clear");
        map.set((16, 16), Some(DistrictId(2)));
        e.end_stroke();
        assert_eq!(
            e.drag(&map, p),
            Some(DistrictCommand::Erase {
                cells: vec![(16, 16)]
            })
        );
    }

    #[test]
    fn no_district_chosen_paints_nothing_and_off_map_has_no_cells() {
        let map = DistrictMap::default();
        let mut t = DistrictTool::default();
        assert_eq!(t.drag(&map, Vec2::new(500.0, 500.0)), None);
        assert!(cells_within(Vec2::new(-5.0, 10.0), 100.0).is_empty());
    }
}

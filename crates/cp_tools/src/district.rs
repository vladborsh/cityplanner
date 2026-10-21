//! The district brush (T-133): paints or clears 32 m cells of the district map.
//!
//! Pure like the zone tool: given the map and a cursor position it says which cells a stroke
//! step would change, and returns the one [`DistrictCommand`] for them; the plugin pushes it.

use std::collections::BTreeSet;

use bevy_math::Vec2;
use cp_district::{
    CELL_M, Cell, DistrictCommand, DistrictId, DistrictMap, Districts, MAX_DISTRICTS,
};
use cp_locale::trf;

/// The id the next `Create` will get: the lowest free slot (the rule of `cp_district`).
pub fn next_district(ds: &Districts) -> Option<DistrictId> {
    let slot = ds
        .list
        .iter()
        .position(Option::is_none)
        .unwrap_or(ds.list.len());
    (slot < MAX_DISTRICTS).then(|| DistrictId(slot as u8 + 1))
}

/// "District N" with `N` the id the new district gets.
pub fn new_district_name(id: DistrictId) -> String {
    trf!("District {n}", n = id.0)
}

/// Cells whose centre is within this many metres of the cursor are in the small brush (and the
/// eraser): the cell under it and its four neighbours, a small round brush (BUG-108).
pub const BRUSH_SMALL_RADIUS: f32 = CELL_M;
/// The same for the big brush: a disk three cells across each way, the square's corners out.
pub const BRUSH_LARGE_RADIUS: f32 = 3.0 * CELL_M;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DistrictToolKind {
    /// The cells within one cell of the cursor.
    #[default]
    BrushSmall,
    /// Cells within three cells of the cursor.
    BrushLarge,
    /// Clears the cells under the small brush.
    Erase,
}

/// Where a brush at `hover` is centred: the centre of the cell under the cursor, so a brush keeps
/// its shape (a disk of whole cells) wherever in the cell the cursor is.
pub fn brush_centre(hover: Vec2) -> Option<Vec2> {
    DistrictMap::cell_of(hover).map(DistrictMap::cell_center)
}

/// The cells a brush of `kind` at `hover` takes: those whose centre is within its radius of
/// [`brush_centre`] (BUG-108: a round brush, the square's corners left out).
pub fn brush_cells(hover: Vec2, kind: DistrictToolKind) -> Vec<Cell> {
    brush_centre(hover).map_or_else(Vec::new, |c| cells_within(c, kind.radius() + 1e-3))
}

impl DistrictToolKind {
    /// The brush's radius (cells by their centre's distance from the cursor; BUG-108: round).
    pub fn radius(self) -> f32 {
        match self {
            Self::BrushLarge => BRUSH_LARGE_RADIUS,
            Self::BrushSmall | Self::Erase => BRUSH_SMALL_RADIUS,
        }
    }

    /// The circle the ghost draws round the cursor: where the painted cells reach.
    pub fn reach(self) -> f32 {
        self.radius() + 0.5 * CELL_M
    }
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

    /// A brush stroke with no district chosen (BUG-108: it painted nothing, without a word):
    /// the first district is chosen, else a new one is made and chosen. Returns the `Create` to
    /// send before the stroke's paint (the two run in order in one tick). The eraser needs none.
    pub fn choose_or_create(&mut self, ds: &Districts) -> Option<DistrictCommand> {
        if self.selected.is_some() || self.kind == DistrictToolKind::Erase {
            return None;
        }
        if let Some((id, _)) = ds.iter().next() {
            self.selected = Some(id);
            return None;
        }
        let id = next_district(ds)?;
        self.selected = Some(id);
        Some(DistrictCommand::Create {
            name: new_district_name(id),
        })
    }

    /// The cells under the brush with the cursor at `hover` (the highlight).
    pub fn preview(&self, hover: Vec2) -> Vec<Cell> {
        brush_cells(hover, self.kind)
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
        let mut small = tool(DistrictToolKind::BrushSmall).preview(p);
        small.sort();
        assert_eq!(
            small,
            vec![(15, 16), (16, 15), (16, 16), (16, 17), (17, 16)]
        );
        // BUG-108: the big brush is a disk of whole cells: radius 3 cells by centre distance.
        let big = tool(DistrictToolKind::BrushLarge).preview(p);
        assert_eq!(big.len(), 29, "the cells within 3 of the centre");
        for c in [(19, 16), (16, 13), (18, 18), (14, 14)] {
            assert!(big.contains(&c), "{c:?} in the disk");
        }
        for c in [(19, 19), (13, 13), (19, 18), (20, 16)] {
            assert!(!big.contains(&c), "{c:?}: a corner of the square is out");
        }
        // The same disk wherever in the cell the cursor is.
        let corner = Vec2::new(16.0 * CELL_M + 31.0, 16.0 * CELL_M + 1.0);
        assert_eq!(tool(DistrictToolKind::BrushLarge).preview(corner), big);
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

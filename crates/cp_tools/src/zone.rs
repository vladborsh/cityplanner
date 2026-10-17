//! The zoning tools: fill a block, brush (small / large), rectangle, eraser.
//!
//! Pure: given the zone snapshot and a cursor position they say which cells would be painted;
//! the plugin turns that into `ZoneCommand::PaintZones`.

use std::collections::BTreeSet;

use bevy_math::Vec2;
use cp_content::ZoneType;
use cp_zone::{ZoneCellRef, ZoneGrid};

pub const BRUSH_SMALL_RADIUS: f32 = 10.0;
pub const BRUSH_LARGE_RADIUS: f32 = 24.0;
pub const ERASER_RADIUS: f32 = 10.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ZoneToolKind {
    /// Click a block: every cell of it.
    #[default]
    FillBlock,
    BrushSmall,
    BrushLarge,
    /// Two clicks: opposite corners of an axis-aligned rectangle.
    Rectangle,
    Eraser,
}

impl ZoneToolKind {
    pub const ALL: [ZoneToolKind; 5] = [
        ZoneToolKind::FillBlock,
        ZoneToolKind::BrushSmall,
        ZoneToolKind::BrushLarge,
        ZoneToolKind::Rectangle,
        ZoneToolKind::Eraser,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::FillBlock => "Fill block",
            Self::BrushSmall => "Brush S",
            Self::BrushLarge => "Brush L",
            Self::Rectangle => "Rectangle",
            Self::Eraser => "Eraser",
        }
    }

    /// Brushes paint continuously while the button is held.
    pub fn is_brush(self) -> bool {
        matches!(self, Self::BrushSmall | Self::BrushLarge | Self::Eraser)
    }

    fn radius(self) -> f32 {
        match self {
            Self::BrushSmall => BRUSH_SMALL_RADIUS,
            Self::BrushLarge => BRUSH_LARGE_RADIUS,
            _ => ERASER_RADIUS,
        }
    }
}

/// The zone types the player can paint, in palette order.
pub const PALETTE: [ZoneType; 6] = [
    ZoneType::ResLow,
    ZoneType::ResHigh,
    ZoneType::ComLow,
    ZoneType::ComHigh,
    ZoneType::Industrial,
    ZoneType::Office,
];

pub fn zone_name(z: ZoneType) -> &'static str {
    match z {
        ZoneType::None => "None",
        ZoneType::ResLow => "Residential (low)",
        ZoneType::ResHigh => "Residential (high)",
        ZoneType::ComLow => "Commercial (low)",
        ZoneType::ComHigh => "Commercial (high)",
        ZoneType::Industrial => "Industrial",
        ZoneType::Office => "Office",
    }
}

/// Overlay colour (sRGB) per zone type: greens residential, blues commercial, yellow
/// industrial, purple office.
pub fn zone_color(z: ZoneType) -> [f32; 3] {
    match z {
        ZoneType::None => [0.55, 0.55, 0.55],
        ZoneType::ResLow => [0.55, 0.85, 0.35],
        ZoneType::ResHigh => [0.10, 0.60, 0.20],
        ZoneType::ComLow => [0.45, 0.70, 0.95],
        ZoneType::ComHigh => [0.15, 0.35, 0.85],
        ZoneType::Industrial => [0.95, 0.80, 0.15],
        ZoneType::Office => [0.70, 0.40, 0.85],
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ZoneTool {
    pub kind: ZoneToolKind,
    /// Type painted by every tool except the eraser.
    pub zone: ZoneType,
    /// First corner of a rectangle in progress.
    pub corner: Option<Vec2>,
    /// Cells already painted in the current brush stroke.
    pub stroke: BTreeSet<ZoneCellRef>,
}

impl Default for ZoneTool {
    fn default() -> Self {
        Self {
            kind: ZoneToolKind::default(),
            zone: ZoneType::ResLow,
            corner: None,
            stroke: BTreeSet::new(),
        }
    }
}

impl ZoneTool {
    /// What this tool paints.
    pub fn paint_type(&self) -> ZoneType {
        if self.kind == ZoneToolKind::Eraser {
            ZoneType::None
        } else {
            self.zone
        }
    }

    pub fn set_kind(&mut self, kind: ZoneToolKind) {
        self.kind = kind;
        self.cancel();
    }

    pub fn cancel(&mut self) {
        self.corner = None;
        self.stroke.clear();
    }

    /// Cells in `cells` that painting would change: valid (or built on: a building that no
    /// longer fits its zone is demolished, BUG-053), not already of the target type.
    fn paintable(&self, zones: &ZoneGrid, cells: Vec<ZoneCellRef>) -> Vec<ZoneCellRef> {
        let want = self.paint_type();
        cells
            .into_iter()
            .filter(|c| {
                (zones.is_valid(*c) || zones.is_occupied(*c)) && zones.zone_of(*c) != Some(want)
            })
            .collect()
    }

    /// The cells the tool would paint with the cursor at `hover` (the highlight).
    pub fn preview(&self, zones: &ZoneGrid, hover: Vec2) -> Vec<ZoneCellRef> {
        let cells = match self.kind {
            ZoneToolKind::FillBlock => zones
                .cell_at(hover)
                .map(|c| zones.block_cells(c.block))
                .unwrap_or_default(),
            ZoneToolKind::Rectangle => match self.corner {
                Some(a) => zones.cells_in_rect(a, hover),
                None => zones.cell_at(hover).into_iter().collect(),
            },
            k => zones.cells_within(hover, k.radius()),
        };
        self.paintable(zones, cells)
    }

    /// A click at `p` (press, for brushes). Returns the cells to paint and the type when the
    /// click completes an action.
    pub fn click(&mut self, zones: &ZoneGrid, p: Vec2) -> Option<(Vec<ZoneCellRef>, ZoneType)> {
        match self.kind {
            ZoneToolKind::Rectangle => match self.corner.take() {
                None => {
                    self.corner = Some(p);
                    None
                }
                Some(a) => {
                    let cells = self.paintable(zones, zones.cells_in_rect(a, p));
                    (!cells.is_empty()).then(|| (cells, self.paint_type()))
                }
            },
            _ => self.drag(zones, p),
        }
    }

    /// The cursor at `p` with the button held (also the press itself): paints the cells not yet
    /// painted in this stroke. Fill-block and rectangle have no strokes, a fill is one click.
    pub fn drag(&mut self, zones: &ZoneGrid, p: Vec2) -> Option<(Vec<ZoneCellRef>, ZoneType)> {
        if self.kind == ZoneToolKind::Rectangle {
            return None;
        }
        let cells: Vec<ZoneCellRef> = self
            .preview(zones, p)
            .into_iter()
            .filter(|c| !self.stroke.contains(c))
            .collect();
        if cells.is_empty() {
            return None;
        }
        if self.kind.is_brush() {
            self.stroke.extend(cells.iter().copied());
        }
        Some((cells, self.paint_type()))
    }

    /// The button was released: the next press starts a new stroke.
    pub fn end_stroke(&mut self) {
        self.stroke.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec3;
    use cp_content::ContentDb;
    use cp_core::curve::CubicBezier3;
    use cp_net::{Elevation, NetGraph};
    use cp_zone::{RoadZoning, command::paint};

    fn grid() -> ZoneGrid {
        let db = ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content"))
            .unwrap();
        let road = db.road_id("basic_2l").unwrap();
        let def = db.road(road).clone();
        let mut net = NetGraph::new();
        net.add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(1000.0, 0.0, 1000.0),
                Vec3::new(1096.0, 0.0, 1000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
        let mut z = ZoneGrid::new();
        z.sync(
            &net,
            &|id| {
                let d = db.road(id);
                RoadZoning {
                    allows_zoning: d.allows_zoning,
                    half_width: d.width / 2.0,
                }
            },
            &|_, s| s.curve,
        );
        z
    }

    /// Right side of the road (+Z): row 0 centre line at z = 1000 + 6 + 4.
    fn right_cell(z: &ZoneGrid, col: usize, row: usize) -> (ZoneCellRef, Vec2) {
        let id = z
            .blocks()
            .find(|(_, b)| b.side == cp_zone::Side::Right)
            .map(|(id, _)| id)
            .unwrap();
        let c = ZoneCellRef {
            block: id,
            cell: (col * 4 + row) as u16,
        };
        (c, z.cell_footprint(c).unwrap().c)
    }

    #[test]
    fn fill_block_paints_the_whole_block_under_the_cursor() {
        let z = grid();
        let (_, p) = right_cell(&z, 3, 1);
        let mut t = ZoneTool {
            kind: ZoneToolKind::FillBlock,
            zone: ZoneType::ComHigh,
            ..Default::default()
        };
        let (cells, zone) = t.click(&z, p).unwrap();
        assert_eq!(zone, ZoneType::ComHigh);
        assert_eq!(cells.len(), 12 * 4);
        assert!(
            cells
                .iter()
                .all(|c| c.block == right_cell(&z, 0, 0).0.block)
        );
        // Off the blocks: nothing.
        assert!(t.click(&z, Vec2::new(0.0, 0.0)).is_none());
    }

    #[test]
    fn brushes_cover_a_radius_and_large_covers_more() {
        let z = grid();
        let (_, p) = right_cell(&z, 5, 0);
        let small = ZoneTool {
            kind: ZoneToolKind::BrushSmall,
            ..Default::default()
        }
        .preview(&z, p);
        let large = ZoneTool {
            kind: ZoneToolKind::BrushLarge,
            ..Default::default()
        }
        .preview(&z, p);
        assert!(
            !small.is_empty() && large.len() > small.len(),
            "{} vs {}",
            small.len(),
            large.len()
        );
        for c in &small {
            assert!(z.cell_footprint(*c).unwrap().c.distance(p) <= BRUSH_SMALL_RADIUS);
        }
    }

    #[test]
    fn a_brush_stroke_paints_each_cell_once_until_released() {
        let z = grid();
        let (_, p) = right_cell(&z, 5, 0);
        let mut t = ZoneTool {
            kind: ZoneToolKind::BrushLarge,
            ..Default::default()
        };
        let (first, _) = t.click(&z, p).unwrap();
        assert!(t.drag(&z, p).is_none(), "same spot, nothing new");
        let (more, _) = t.drag(&z, p + Vec2::new(16.0, 0.0)).unwrap();
        assert!(more.iter().all(|c| !first.contains(c)));
        t.end_stroke();
        assert!(
            t.click(&z, p).is_some(),
            "a new stroke may paint those cells again"
        );
    }

    #[test]
    fn rectangle_takes_two_corners_and_previews_with_the_hover() {
        let z = grid();
        let (_, a) = right_cell(&z, 1, 0);
        let (_, b) = right_cell(&z, 4, 1);
        let mut t = ZoneTool {
            kind: ZoneToolKind::Rectangle,
            zone: ZoneType::Industrial,
            ..Default::default()
        };
        assert!(t.click(&z, a).is_none());
        let shown = t.preview(&z, b);
        let (cells, zone) = t.click(&z, b).unwrap();
        assert_eq!(zone, ZoneType::Industrial);
        assert_eq!(shown, cells, "the preview is what the second click paints");
        // Columns 1..=4 x rows 0..=1 of the right block, minus any invalid cell.
        let valid = z
            .cells_in_rect(a, b)
            .into_iter()
            .filter(|c| z.is_valid(*c))
            .count();
        assert_eq!(cells.len(), valid);
        assert!((6..=8).contains(&cells.len()), "{}", cells.len());
        assert!(t.corner.is_none());
    }

    #[test]
    fn eraser_only_targets_painted_cells_and_paints_none() {
        let mut z = grid();
        let (c, p) = right_cell(&z, 5, 0);
        let t = ZoneTool {
            kind: ZoneToolKind::Eraser,
            ..Default::default()
        };
        assert!(t.preview(&z, p).is_empty(), "nothing painted yet");
        paint(&mut z, &[c], ZoneType::ResHigh).unwrap();
        let mut t = t;
        let (cells, zone) = t.click(&z, p).unwrap();
        assert_eq!(zone, ZoneType::None);
        assert_eq!(cells, vec![c]);
    }

    #[test]
    fn already_painted_cells_are_not_repainted_with_the_same_type() {
        let mut z = grid();
        let (c, p) = right_cell(&z, 5, 0);
        paint(&mut z, &[c], ZoneType::ResLow).unwrap();
        let t = ZoneTool {
            kind: ZoneToolKind::BrushSmall,
            zone: ZoneType::ResLow,
            ..Default::default()
        };
        assert!(!t.preview(&z, p).contains(&c));
        let t = ZoneTool {
            zone: ZoneType::Office,
            ..t
        };
        assert!(t.preview(&z, p).contains(&c));
    }

    #[test]
    fn palette_has_distinct_colours_and_names() {
        let cols: BTreeSet<[u32; 3]> = PALETTE
            .iter()
            .map(|z| zone_color(*z).map(|c| (c * 1000.0) as u32))
            .collect();
        assert_eq!(cols.len(), PALETTE.len());
        assert!(PALETTE.iter().all(|z| !zone_name(*z).is_empty()));
    }
}

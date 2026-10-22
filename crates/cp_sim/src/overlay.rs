//! Overlay data for clients (`architecture/system-design.md` §13.4): what the traffic,
//! zone, land value and pollution overlays colour. Plain read-only snapshots inside
//! [`SimView`](crate::SimView); the sim itself never reads them.

use std::collections::BTreeMap;
use std::sync::Arc;

use cp_content::ZoneType;
use cp_net::SegmentId;
use cp_zone::ZoneGrid;

/// One zone cell as the zone overlay draws it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoneCellView {
    /// XZ corners of the footprint, in outline order.
    pub corners: [[f32; 2]; 4],
    pub zone: ZoneType,
    /// Not blocked by a road, another block or a building. Invalid cells are never painted.
    pub valid: bool,
}

/// Every zone cell, flat, in block-id then cell order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZoneOverlay {
    pub cells: Vec<ZoneCellView>,
}

/// A square grid of normalised values `0..=1` over the whole map, row-major (`z * side + x`),
/// for the heat-map overlays.
#[derive(Clone, Debug, PartialEq)]
pub struct ScalarGrid {
    pub side: usize,
    pub values: Vec<f32>,
}

impl ScalarGrid {
    pub fn new(side: usize, values: Vec<f32>) -> Self {
        assert_eq!(values.len(), side * side, "grid is side x side");
        Self { side, values }
    }
}

/// The overlay inputs of one snapshot. Land value and pollution (T-042) fill their grids when
/// that domain lands; `road_load` comes from the traffic simulation.
#[derive(Clone, Debug, Default)]
pub struct OverlayData {
    pub zones: Arc<ZoneOverlay>,
    /// Load of each segment, `0..=1` of its capacity; absent segments have no data.
    pub road_load: Arc<BTreeMap<SegmentId, f32>>,
    pub land_value: Option<Arc<ScalarGrid>>,
    pub pollution: Option<Arc<ScalarGrid>>,
    /// Noise (T-300), `0..=1`.
    pub noise: Option<Arc<ScalarGrid>>,
}

/// `(land grids revision, land value, pollution, noise)` of the last snapshot.
pub(crate) type LandCache = Option<(u64, [Arc<ScalarGrid>; 3])>;

/// `(zone grid revision, overlay)` of the last snapshot.
pub(crate) type ZoneCache = Option<(u64, Arc<ZoneOverlay>)>;

/// The zone overlay of `grid`, reusing `cache` while the zone state is unchanged.
pub(crate) fn zone_snapshot(grid: Option<&ZoneGrid>, cache: &mut ZoneCache) -> Arc<ZoneOverlay> {
    let Some(grid) = grid else {
        return Arc::default();
    };
    // `revision` changes with every edit of the grid (paint, rebuild, validity, occupancy).
    let key = grid.revision();
    if let Some((k, snap)) = cache.as_ref()
        && *k == key
    {
        return Arc::clone(snap);
    }
    let snap = Arc::new(build_zone_overlay(grid));
    *cache = Some((key, Arc::clone(&snap)));
    snap
}

/// Flattens the zone grid into drawable cells.
pub fn build_zone_overlay(grid: &ZoneGrid) -> ZoneOverlay {
    let mut cells = Vec::new();
    for (_, block) in grid.blocks() {
        for (n, obb) in block.footprints().iter().enumerate() {
            cells.push(ZoneCellView {
                corners: obb.corners().map(|c| [c.x, c.y]),
                zone: block.zones[n],
                valid: block.valid[n],
            });
        }
    }
    ZoneOverlay { cells }
}

//! The Happiness overlay (T-318c, `architecture/system-design.md` §19.5): every home with a
//! contentment record tinted by its index, red (miserable) → yellow → green (content); other
//! buildings are not tinted. A pure function of the sim snapshot, like the other layers.

use cp_building::{BuildingGrid, BuildingState, HomeContentment};
use cp_terrain::Terrain;
use cp_zone::ZoneGrid;

use super::ramp;
use super::utility::lot_quads;
use crate::chunk::MeshData;

/// The tint of a home of contentment `index` (`0..=100`).
pub fn happiness_color(index: u8) -> [f32; 4] {
    ramp(
        f32::from(index.min(100)) / 100.0,
        &[
            [0.85, 0.20, 0.15, 0.80],
            [0.95, 0.80, 0.15, 0.80],
            [0.20, 0.75, 0.25, 0.80],
        ],
    )
}

/// The lots of the active homes with a record, each in its index's tint.
pub fn happiness_overlay_mesh(
    homes: &HomeContentment,
    buildings: &BuildingGrid,
    zones: &ZoneGrid,
    terrain: &Terrain,
) -> MeshData {
    let mut m = MeshData::default();
    for (id, slot) in homes.homes() {
        let Some(b) = buildings.get(id) else { continue };
        if b.state != BuildingState::Active {
            continue;
        }
        lot_quads(&mut m, b, zones, terrain, happiness_color(slot.index));
    }
    m
}

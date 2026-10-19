//! Format 56 (T-265, layout v3): cells are 8 m, never stretched to fit a block. No serialized
//! type changed, so the saved blocks are laid again on load (like v46 and v54). The v55 fixture
//! (made by the real v55 writer: a closed 70 m block whose cells were fitted to 8.75 m, painted
//! and grown) loads; its first sync lays every block again in 8 m cells, the painted types are
//! carried over by position, and no building is demolished for it: one whose lot does not land
//! on whole new cells keeps standing.

use std::path::Path;
use std::sync::Arc;

use cp_building::BuildingGrid;
use cp_content::{ContentDb, ZoneType};
use cp_save::*;
use cp_sim::SimRunner;
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn assets() -> (Arc<ContentDb>, Arc<Terrain>) {
    (
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap()),
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap()),
    )
}

#[test]
fn the_v55_fitted_block_is_laid_again_in_8_m_cells_and_keeps_its_buildings() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v55-fitted.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 55, "made by the v55 writer");
    assert_eq!(FORMAT_VERSION, 56);
    let (content, terrain) = assets();

    // As saved: fitted cells.
    let loaded = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let fitted = loaded
        .data
        .zones
        .blocks()
        .filter(|(_, b)| (b.cell_len - 8.0).abs() > 0.01)
        .count();
    assert!(fitted > 0, "the fixture has fitted cells");

    let mut sim = SimRunner::new(7);
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&content));
    sim.tick_once();
    load_into_runner(&mut sim, &bytes, content.content_hash()).unwrap();
    let before: Vec<_> = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .map(|(id, _)| id)
        .collect();
    assert!(before.len() >= 8, "{} buildings", before.len());
    for _ in 0..1000 {
        sim.tick_once();
    }
    let view = sim.view();
    // Every block laid again in 8 m cells.
    assert!(view.zones.block_count() > 0);
    for (_, b) in view.zones.blocks() {
        assert!(
            (b.cell_len - 8.0).abs() < 1e-6 && (b.row_depth - 8.0).abs() < 1e-6,
            "a block of {} x {} m cells",
            b.cell_len,
            b.row_depth
        );
    }
    // The painted type carried over: the new cells are residential.
    let painted = view
        .zones
        .blocks()
        .flat_map(|(_, b)| b.zones.iter())
        .filter(|z| **z == ZoneType::ResLow)
        .count();
    assert!(painted > 0, "no painted cell after the relay");
    // No building was demolished by the relay; some lost their lot (the 8.75 m cells under them
    // are gone and the new 8 m ones do not make the lot's rectangle) and stand all the same.
    let buildings = sim.world().resource::<BuildingGrid>();
    for id in &before {
        assert!(
            buildings.get(*id).is_some(),
            "building {id:?} was demolished"
        );
    }
    let unbound = buildings
        .iter()
        .filter(|(_, b)| b.lot().is_some_and(|l| view.zones.block(l.block).is_none()))
        .count();
    assert!(
        unbound > 0 && unbound < before.len(),
        "{unbound} of {} buildings lost their lot",
        before.len()
    );

    // The saved again file is the current format and loads back the same.
    let again = save_runner(&sim, content.content_hash(), "flatlands").unwrap();
    assert_eq!(read_header(&again).unwrap().0.format_version, 56);
    let (content2, terrain2) = assets();
    let mut sim2 = SimRunner::new(7);
    sim2.set_terrain(terrain2);
    sim2.set_content(content2);
    sim2.tick_once();
    load_into_runner(&mut sim2, &again, content.content_hash()).unwrap();
    sim2.tick_once();
    sim.tick_once();
    assert_eq!(sim.checksum(), sim2.checksum());
}

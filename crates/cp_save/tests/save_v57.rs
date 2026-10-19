//! Format 57 (T-275, whole cells): a dead end keeps a half width free for the cross street that
//! will close it, and the frontages facing each other across a block count from the same end
//! (§6.8.3 item 4 (г), (д)). No serialized type changed, so the saved blocks are laid again on
//! load (like v46, v54 and v56). The v56 fixture (made by the real v56 writer: a lone 100 m street
//! whose cells ran to both dead ends, painted and grown) loads; its first sync lays every block
//! again, the painted types are carried over by position, and no building is demolished for it.

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

fn columns(zones: &cp_zone::ZoneGrid) -> u32 {
    zones.blocks().map(|(_, b)| u32::from(b.cols)).sum()
}

#[test]
fn the_v56_street_is_laid_again_off_its_dead_ends_and_keeps_its_buildings() {
    let bytes =
        std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v56-dead-ends.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 56, "made by the v56 writer");
    assert_eq!(FORMAT_VERSION, 57);
    let (content, terrain) = assets();

    // As saved: 12 columns a side, right up to the dead ends.
    let loaded = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(columns(&loaded.data.zones), 24);

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
    assert!(before.len() >= 6, "{} buildings", before.len());
    for _ in 0..1000 {
        sim.tick_once();
    }
    let view = sim.view();
    // Laid again: 100 m less a half width at each dead end, 88 m, 11 columns a side.
    assert_eq!(columns(&view.zones), 22);
    let painted = view
        .zones
        .blocks()
        .flat_map(|(_, b)| b.zones.iter())
        .filter(|z| **z == ZoneType::ResLow)
        .count();
    assert!(painted > 0, "no painted cell after the relay");
    let buildings = sim.world().resource::<BuildingGrid>();
    for id in &before {
        assert!(
            buildings.get(*id).is_some(),
            "building {id:?} was demolished"
        );
    }

    // Saved again: the current format, and it loads back the same.
    let again = save_runner(&sim, content.content_hash(), "flatlands").unwrap();
    assert_eq!(read_header(&again).unwrap().0.format_version, 57);
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

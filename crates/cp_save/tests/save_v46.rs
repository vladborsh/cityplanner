//! Format 46 (BUG-047): zone layout v2. The v45 fixture (made by the real v45 writer, a painted
//! and built grid city) loads, is laid out again on the first ticks, keeps most painted
//! cells and buildings, and replays deterministically.

use std::path::Path;
use std::sync::Arc;

use cp_building::BuildingGrid;
use cp_content::{ContentDb, ZoneType};
use cp_save::*;
use cp_sim::{SimRunner, SimTerrain};
use cp_terrain::Terrain;
use cp_zone::ZoneGrid;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn runner(content: &Arc<ContentDb>) -> SimRunner {
    let terrain = Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap();
    let mut sim = SimRunner::new(5);
    sim.set_terrain(Arc::new(terrain));
    sim.set_content(Arc::clone(content));
    sim
}

fn loaded(bytes: &[u8], content: &Arc<ContentDb>) -> SimRunner {
    let mut sim = runner(content);
    load_into_runner(&mut sim, bytes, content.content_hash()).unwrap();
    assert!(sim.world().get_resource::<SimTerrain>().is_some());
    sim
}

#[test]
fn the_v45_fixture_is_laid_out_again_and_keeps_its_paint_and_buildings() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v45-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 45, "made by the v45 writer");
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());

    // The painted valid cells of the old layout, by centre.
    let old = load_with_map(&bytes, content.content_hash(), h.map_hash)
        .unwrap()
        .data;
    let painted: Vec<_> = old
        .zones
        .blocks()
        .flat_map(|(id, _)| old.zones.block_cells(id))
        .filter(|c| old.zones.is_valid(*c) && old.zones.zone_of(*c) != Some(ZoneType::None))
        .map(|c| {
            (
                old.zones.cell_footprint(c).unwrap().c,
                old.zones.zone_of(c).unwrap(),
            )
        })
        .collect();
    assert!(
        painted.len() > 50,
        "the fixture is painted: {}",
        painted.len()
    );
    let old_buildings = old.buildings.iter().count();
    assert!(
        old_buildings > 5,
        "the fixture is built on: {old_buildings}"
    );

    let mut sim = loaded(&bytes, &content);
    for _ in 0..3 {
        sim.tick_once();
    }
    let zones = sim.world().resource::<ZoneGrid>();
    let kept = painted
        .iter()
        .filter(|(p, z)| {
            zones
                .cell_at(*p)
                .is_some_and(|c| zones.is_valid(c) && zones.zone_of(c) == Some(*z))
        })
        .count();
    // What is lost is the deep rows at block corners, where two blocks' cells overlap and one
    // wins (rank rule, §7.1), plus a few end cells on a cross street; the old layout trimmed
    // those with the junction geometry. Almost nothing else.
    let (mut no_cell, mut loser, mut other_type, mut stray) = (0, 0, 0, 0);
    for (p, z) in &painted {
        match zones.cell_at(*p) {
            None => no_cell += 1,
            Some(c) if !zones.is_valid(c) => {
                let o = zones.cell_footprint(c).unwrap();
                let beaten = zones
                    .cells_within(o.c, 14.0)
                    .into_iter()
                    .any(|r| r.block != c.block && zones.is_valid(r));
                if beaten {
                    loser += 1;
                } else {
                    stray += 1;
                }
            }
            Some(c) if zones.zone_of(c) != Some(*z) => other_type += 1,
            _ => {}
        }
    }
    assert!(
        no_cell <= 5 && stray * 50 <= painted.len(),
        "{no_cell} no cell, {stray} stray"
    );
    assert!(
        kept * 3 >= painted.len() * 2,
        "{kept} of {} painted cells keep their type ({loser} lose a corner, {other_type} other type)",
        painted.len()
    );

    // Every building stands on cells it occupies.
    let buildings = sim.world().resource::<BuildingGrid>();
    for (id, b) in buildings.iter() {
        for c in cp_building::held_cells(zones, b) {
            assert!(zones.is_occupied(c), "{id:?} lot cell {c:?} is occupied");
        }
    }
    assert!(
        buildings.iter().count() * 10 >= old_buildings * 9,
        "buildings survive"
    );

    // Determinism: a second run from the same file ends in the same state.
    for _ in 0..50 {
        sim.tick_once();
    }
    let mut again = loaded(&bytes, &content);
    for _ in 0..53 {
        again.tick_once();
    }
    assert_eq!(sim.checksum(), again.checksum());

    // And the file written now is v46 and loads back to the same data.
    let saved = save_runner(&sim, h.content_hash, &h.map_id).unwrap();
    assert_eq!(
        read_header(&saved).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l = load_with_map(&saved, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, SaveData::capture(sim.world()));
}

//! Format 35 (T-185): zone blocks carry the length and depth of their cells. The v34 fixture
//! (made by the real v34 writer) loads with standard cells everywhere and round-trips on the
//! new version.

use std::path::Path;

use cp_content::ContentDb;
use cp_save::*;
use cp_zone::CELL_SIZE;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v34_fixture_loads_with_standard_cells_and_round_trips_on_the_new_version() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v34-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 34, "made by the v34 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let blocks: Vec<_> = l.data.zones.blocks().collect();
    assert!(!blocks.is_empty(), "the fixture has zone blocks");
    assert!(
        blocks
            .iter()
            .all(|(_, b)| b.cell_len == CELL_SIZE && b.row_depth == CELL_SIZE)
    );
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
}

//! Format 38 (T-200): unique buildings. The v37 fixture (made by the real v37 writer, milestone
//! 1 reached) loads with nothing opened, and a progress with an opened unique building
//! round-trips.

use std::path::Path;

use cp_content::ContentDb;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v37_fixture_loads_with_no_unique_building_open_and_round_trips_with_one() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v37-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 37, "made by the v37 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data.progress.reached, 1);
    assert!(l.data.progress.unlocked.is_empty());

    l.data.progress.unlocked.insert("city_plaza".to_string());
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert!(l2.data.progress.unlocked.contains("city_plaza"));
}

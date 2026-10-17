//! Format 47 (T-223): park state. The v46 fixture (made by the real v46 writer, a grid city with
//! a city-park area) loads with an empty park state and its area kept, and a park state survives
//! a save and a load.

use std::path::Path;

use cp_content::ContentDb;
use cp_park::ParkAreaState;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v46_fixture_loads_with_no_park_state_and_a_park_state_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v46-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 46, "made by the v46 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.parks.is_empty());
    assert_eq!(l.data.areas.list.len(), 1, "the park area is kept");

    l.data.parks.areas.insert(
        1,
        ParkAreaState {
            attract: 34.1,
            capacity: 120,
            inside: 7,
            visitors_day: 40,
            visitors_week: 200,
            fee: 2.5,
        },
    );
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.parks.areas[&1].capacity, 120);
}

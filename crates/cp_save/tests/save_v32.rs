//! Format 32 (T-160): the week report carries the transit fares and upkeep. The v31 fixture
//! (made by the real v31 writer) loads with zeros there and round-trips on the new version.

use std::path::Path;

use cp_content::ContentDb;
use cp_core::Cost;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v31_fixture_loads_with_zero_transit_money_and_round_trips_on_the_new_version() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v31-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 31, "made by the v31 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let w = l.data.economy.last_week;
    assert_eq!(
        (w.transit_fares, w.transit_upkeep),
        (Cost::ZERO, Cost::ZERO)
    );
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
}

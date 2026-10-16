//! Format 33 (T-168): tourists — the week report and the ledger carry the tourism money and the
//! save carries the visits. The v32 fixture (made by the real v32 writer) loads with none of it
//! and round-trips on the new version.

use std::path::Path;

use cp_content::ContentDb;
use cp_core::Cost;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v32_fixture_loads_without_tourists_and_round_trips_on_the_new_version() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v32-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 32, "made by the v32 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.tourists.is_empty());
    assert_eq!(l.data.ledger.tourism, Cost::ZERO);
    assert_eq!(l.data.economy.last_week.tourism, Cost::ZERO);
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
}

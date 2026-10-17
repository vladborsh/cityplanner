//! Format 37 (T-170): goods trade. The v36 fixture (made by the real v36 writer, a grid city run
//! until a goods truck was on the road) loads with its truck as a factory-to-shop haul and no
//! exports, and round-trips on the new version.

use std::path::Path;

use cp_content::ContentDb;
use cp_core::Cost;
use cp_save::*;
use cp_transit::Endpoint;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v36_fixture_loads_its_truck_as_a_haul_between_buildings_and_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v36-goods.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 36, "made by the v36 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let goods = &l.data.goods;
    assert_eq!(goods.hauls.len(), 1);
    assert!(goods.stock.len() > 10);
    for haul in goods.hauls.values() {
        assert!(matches!(haul.from, Endpoint::Building(_)));
        assert!(matches!(haul.to, Endpoint::Building(_)));
        assert!(haul.load > 0.0);
    }
    assert_eq!(l.data.economy.last_week.exports, Cost::ZERO);
    assert_eq!(l.data.ledger.exports, Cost::ZERO);
    assert_eq!(l.data.ledger.export_loads + l.data.ledger.import_loads, 0);
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
}

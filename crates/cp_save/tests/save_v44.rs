//! Format 44 (T-210): cargo trucks. The v43 fixture (made by the real v43 writer, a grid city run
//! until a goods truck was on the road) loads with its truck as a haul of shop goods, and the
//! cargo and the trade ledger survive a save and a load.

use std::path::Path;

use cp_content::{CargoKind, ContentDb};
use cp_core::Cost;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v43_fixture_loads_its_trucks_as_goods_and_the_ledger_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v43-goods.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 43, "made by the v43 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(!l.data.goods.hauls.is_empty());
    for haul in l.data.goods.hauls.values() {
        assert_eq!(haul.cargo, CargoKind::Goods);
        assert!(haul.load > 0.0);
    }
    assert!(l.data.industry_ledger.is_empty());

    l.data.industry_ledger.exports = Cost::from_units(140.0);
    l.data.industry_ledger.export_loads = 1;
    if let Some(h) = l.data.goods.hauls.values_mut().next() {
        h.cargo = CargoKind::Logs;
    }
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert!(
        l2.data
            .goods
            .hauls
            .values()
            .any(|h| h.cargo == CargoKind::Logs)
    );
}

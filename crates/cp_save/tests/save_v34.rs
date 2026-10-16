//! Format 34 (T-169): goods — the save carries the stock of factories and shops and the trucks
//! on the road. The v33 fixture (made by the real v33 writer) loads with none of it and
//! round-trips on the new version.

use std::path::Path;

use cp_content::ContentDb;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v33_fixture_loads_without_goods_and_round_trips_on_the_new_version() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v33-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 33, "made by the v33 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.goods.is_empty());
    let mut data = l.data.clone();
    let shop = cp_building::BuildingId::new(1, 0);
    data.goods.stock.insert(shop, 12.5);
    let again = save_with_map(&data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(data, l2.data);
}

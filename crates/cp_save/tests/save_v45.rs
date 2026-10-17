//! Format 45 (T-214): warehouse settings. The v44 fixture (made by the real v44 writer, a grid
//! city with a forestry area) loads with no settings, and a setting survives a save and a load.

use std::path::Path;

use cp_building::BuildingId;
use cp_content::{CargoKind, ContentDb};
use cp_industry::{WarehouseMode, WarehouseSetting};
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v44_fixture_loads_with_no_warehouse_settings_and_a_setting_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v44-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 44, "made by the v44 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.warehouses.list.is_empty());
    assert!(l.data.areas.list.len() == 1, "the area is kept");

    let w = BuildingId::new(3, 0);
    l.data.warehouses.list.insert(
        w,
        WarehouseSetting {
            cargo: Some(CargoKind::Logs),
            mode: WarehouseMode::Empty,
        },
    );
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.warehouses.get(w).mode, WarehouseMode::Empty);
}

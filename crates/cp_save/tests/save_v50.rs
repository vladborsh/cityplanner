//! Format 50 (T-216): the cargo stations' stock and orders. The v49 fixture (made by the real v49
//! writer, a grid city with a city-park area and a park-fee command) loads with no hubs, and a
//! station's stock and an order survive a save and a load.

use std::path::Path;

use cp_building::BuildingId;
use cp_content::{CargoKind, ContentDb};
use cp_core::Tick;
use cp_save::*;
use cp_transit::CargoHubs;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v49_fixture_loads_with_no_cargo_hubs_and_a_hub_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v49-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 49, "made by the v49 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.hubs.is_empty());
    assert_eq!(l.data.areas.list.len(), 1, "the park area is kept");

    let station = BuildingId::new(7, 0);
    let mut hubs = CargoHubs::default();
    hubs.deposit(station, CargoKind::Logs, 20.0, Tick(100), 2048);
    hubs.deposit(station, CargoKind::Ore, 40.0, Tick(100), 2048);
    hubs.order(
        station,
        CargoKind::Planks,
        BuildingId::new(9, 0),
        20.0,
        Tick(100),
        2048,
    );
    l.data.hubs = hubs.clone();
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    assert_eq!(FORMAT_VERSION, 51);
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.hubs, hubs);
    assert_eq!(l2.data.hubs.hubs[&station].out_total(), 60.0);
}

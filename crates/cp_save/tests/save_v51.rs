//! Format 51 (T-217): a cargo port's ship. The v50 fixture (made by the real v50 writer, with a
//! cargo station holding export stock and an import order) loads with the stock and the order
//! kept and no ship berthed, and a berthed ship survives a save and a load.

use std::path::Path;

use cp_content::ContentDb;
use cp_core::Tick;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v50_fixture_loads_with_its_hub_and_no_ship_and_a_ship_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v50-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 50, "made by the v50 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data.hubs.hubs.len(), 1, "the station is kept");
    let (id, hub) = l.data.hubs.hubs.iter().next().unwrap();
    let id = *id;
    assert!(hub.out_total() > 0.0 && !hub.orders.is_empty());
    assert_eq!(hub.ship, None);
    assert!(hub.train.is_none());

    l.data.hubs.hubs.get_mut(&id).unwrap().ship = Some(cp_transit::ShipVisit {
        phase: cp_transit::ShipPhase::Docked,
        until: Tick(777),
    });
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    assert_eq!(FORMAT_VERSION, 55);
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(
        l2.data.hubs.hubs[&id].ship.map(|s| s.until),
        Some(Tick(777))
    );
}

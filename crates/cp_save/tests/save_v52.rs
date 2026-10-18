//! Format 52 (T-216b): cargo trains and ships as vehicles. The v51 fixture (made by the real v51
//! writer, a port hub with a berthed ship and an import order) loads with the ship standing
//! docked until the same tick and no train, and a station's train and premium round-trip.

use std::path::Path;

use cp_content::ContentDb;
use cp_core::Tick;
use cp_save::*;
use cp_transit::{HubTrain, ShipPhase, TrainPhase};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v51_fixture_loads_with_a_docked_ship_and_a_train_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v51-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 51, "made by the v51 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let (&id, hub) = l.data.hubs.hubs.iter().next().expect("the hub is kept");
    let ship = hub.ship.expect("the ship stands on");
    assert_eq!((ship.phase, ship.until), (ShipPhase::Docked, Tick(5000)));
    assert!(hub.train.is_none() && hub.premium == 0.0);
    assert!(hub.out_total() > 0.0 && !hub.orders.is_empty());

    let hub = l.data.hubs.hubs.get_mut(&id).unwrap();
    hub.train = Some(HubTrain {
        run: 7,
        phase: TrainPhase::Dwell { until: Tick(900) },
    });
    hub.premium = 12.5;
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    assert_eq!(FORMAT_VERSION, 53);
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.hubs.hubs[&id].premium, 12.5);
}

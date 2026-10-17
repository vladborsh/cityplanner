//! Format 49 (T-225): the park fee command. The v48 fixture (made by the real v48 writer, a grid
//! city with a city-park area) loads, and a `SimCommand::Park(SetFee)` in the queue and a fee in
//! the park state survive a save and a load.

use std::path::Path;

use cp_content::ContentDb;
use cp_core::Tick;
use cp_district::AreaId;
use cp_park::ParkCommand;
use cp_save::*;
use cp_sim::SimCommand;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v48_fixture_loads_and_a_fee_command_and_fee_round_trip() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v48-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 48, "made by the v48 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data.areas.list.len(), 1, "the park area is kept");
    assert!(l.data.parks.areas.values().all(|a| a.fee == 0.0));

    let area = AreaId(1);
    let at = Tick(l.data.tick.0 + 10);
    l.data
        .queue
        .push(SimCommand::Park(ParkCommand::SetFee { area, fee: 6.5 }), at);
    l.data.parks.areas.entry(area.0).or_default().fee = 6.5;
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.parks.areas[&area.0].fee, 6.5);
}

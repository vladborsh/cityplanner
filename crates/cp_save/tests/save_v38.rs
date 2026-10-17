//! Format 38 (T-190): policies. The v37 fixture (made by the real v37 writer, a grid city with a
//! painted district) loads with no policy on anywhere and a zero policies line, and the masks
//! survive a save and a load.

use std::path::Path;

use cp_content::{ContentDb, PolicyKind};
use cp_core::Cost;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v37_fixture_loads_with_no_policies_and_round_trips_with_masks() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v37-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 37, "made by the v37 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let districts: Vec<_> = l.data.districts.iter().collect();
    assert_eq!(districts.len(), 1);
    assert_eq!(districts[0].1.name, "Old Town");
    assert_eq!(districts[0].1.policies, 0);
    assert_eq!(l.data.city_policies.mask, 0);
    assert_eq!(l.data.economy.last_week.policies, Cost::ZERO);

    l.data.districts.list[0].as_mut().unwrap().policies = PolicyKind::Recycling.bit();
    l.data.city_policies.mask = PolicyKind::TourismPromotion.bit() | PolicyKind::FreeTransit.bit();
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.city_policies.mask, 0b1100_0000);
}

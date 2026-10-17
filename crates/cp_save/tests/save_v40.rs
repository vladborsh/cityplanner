//! Format 40 (T-196): district specialisations. The v39 fixture (made by the real v39 writer, a
//! grid city with a district that has Recycling on and a city policy) loads with its policies
//! and no specialisation, and a specialisation survives a save and a load.

use std::path::Path;

use cp_content::{ContentDb, PolicyKind, SpecKind};
use cp_district::DistrictId;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v39_fixture_loads_with_its_policies_and_no_specialisation_and_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v39-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 39, "made by the v39 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let d = l.data.districts.get(DistrictId(1)).unwrap();
    assert_eq!(d.policies, PolicyKind::Recycling.bit());
    assert_eq!(d.spec, [None; 6]);
    assert_eq!(
        l.data.city_policies.mask,
        PolicyKind::TourismPromotion.bit()
    );

    l.data.districts.get_mut(DistrictId(1)).unwrap().spec[2] = Some(SpecKind::Tourism);
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(
        l2.data.districts.get(DistrictId(1)).unwrap().spec[2],
        Some(SpecKind::Tourism)
    );
}

//! Format 42 (T-206): industry and park areas. The v41 fixture (made by the real v41 writer, a
//! grid city with a district and spent ore) loads with no areas, keeps its deposits, and areas
//! painted afterwards survive a save and a load.

use std::path::Path;

use cp_content::{ContentDb, ParkKind, ResourceKind};
use cp_district::{AreaId, AreaKind, DistrictId};
use cp_save::*;
use cp_terrain::ResourceDeposits;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v41_fixture_loads_with_no_areas_and_areas_round_trip() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v41-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 41, "made by the v41 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.areas.list.is_empty());
    assert_eq!(l.data.area_map, Default::default());
    assert!(l.data.districts.get(DistrictId(1)).is_some());
    let d: &ResourceDeposits = &l.data.deposits;
    assert!(!d.is_unfilled(), "deposits of v41 are kept");

    l.data.areas.list.push(Some(cp_district::Area {
        name: "Forest 1".into(),
        kind: AreaKind::Industry(ResourceKind::Forestry),
        level: 2,
        xp: 17,
    }));
    l.data.areas.list.push(Some(cp_district::Area {
        name: "Zoo".into(),
        kind: AreaKind::Park(ParkKind::Zoo),
        level: 1,
        xp: 0,
    }));
    l.data.area_map.set((10, 12), Some(AreaId(1)));
    l.data.area_map.set((11, 12), Some(AreaId(2)));
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.area_map.get((11, 12)), Some(AreaId(2)));
}

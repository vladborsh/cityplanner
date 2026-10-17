//! Format 43 (T-209): the stock and problems of the chain buildings. The v42 fixture (made by
//! the real v42 writer, a grid city with a forestry area) loads with no stock, and stock and
//! problems survive a save and a load.

use std::path::Path;

use cp_building::BuildingId;
use cp_content::{CargoKind, ContentDb};
use cp_district::AreaId;
use cp_industry::IndustryProblem;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v42_fixture_loads_with_no_stock_and_stock_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v42-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 42, "made by the v42 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.industry.is_empty());
    assert!(l.data.areas.get(AreaId(1)).is_some(), "the area is kept");

    let b = BuildingId::new(7, 0);
    l.data
        .industry
        .stock
        .insert(b, vec![(CargoKind::Logs, 12.5), (CargoKind::Planks, 3.0)]);
    l.data.industry.problems.insert(b, IndustryProblem::NoInput);
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.industry.stock_of(b, CargoKind::Logs), 12.5);
    assert_eq!(l2.data.industry.problem_of(b), IndustryProblem::NoInput);
}

//! Format 25 (T-149): the milestone progress. The v24 fixture (made by the real v24 writer)
//! loads with every milestone reached and round-trips on the new version with a lower one.

use std::path::Path;

use cp_content::ContentDb;
use cp_save::*;
use cp_sim::Progress;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v24_fixture_loads_with_all_milestones_and_round_trips_with_a_lower_one() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v24-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 24, "made by the v24 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data.progress, Progress::ALL);
    l.data.progress = Progress { reached: 2 };
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.progress.reached, 2);
}

//! Format 36 (T-167): the intercity train. The v35 fixture (made by the real v35 writer, a new
//! game with a station on the railway) loads with no train on the line and round-trips on the
//! new version.

use std::path::Path;

use cp_content::ContentDb;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v35_fixture_loads_without_a_train_and_round_trips_on_the_new_version() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v35-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 35, "made by the v35 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.intercity.is_empty());
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
}

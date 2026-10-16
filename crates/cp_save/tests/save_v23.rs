//! Format 23 (T-145): the `NO_WORKERS` timers are saved. The v22 fixture (made by the real v22
//! writer) loads with none and round-trips on the new version; a hand-set timer survives a
//! save and load.

use std::path::Path;

use cp_content::ContentDb;
use cp_core::Tick;
use cp_economy::WorkerShortage;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v22_fixture_loads_with_no_shortage_timers_and_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v22-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 22, "made by the v22 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.citizens.citizen_count() > 0);
    assert_eq!(l.data.shortage, WorkerShortage::default());
    let id = l.data.buildings.iter().next().unwrap().0;
    l.data.shortage.since.insert(id, Tick(512));
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    assert_eq!(FORMAT_VERSION, 23);
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.shortage.since.get(&id), Some(&Tick(512)));
}

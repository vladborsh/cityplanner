//! Format 24 (T-142): a fire names the station whose engine is on it. The v23 fixture (made by
//! the real v23 writer) loads with no fires and round-trips on the new version; a fire with a
//! station survives a save and load.

use std::path::Path;

use cp_content::ContentDb;
use cp_core::Tick;
use cp_save::*;
use cp_service::{Fire, ServiceProblems};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v23_fixture_loads_without_fires_and_round_trips_with_a_station() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v23-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 23, "made by the v23 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.citizens.citizen_count() > 0);
    assert!(
        l.data
            .service_state
            .records()
            .all(|(_, s)| s.fire.is_none())
    );
    let ids: Vec<_> = l.data.buildings.iter().map(|(id, _)| id).take(2).collect();
    let s = l.data.service_state.get_mut(ids[0]);
    s.fire = Some(Fire {
        since: Tick(100),
        out_at: Some(Tick(900)),
        station: Some(ids[1]),
    });
    s.problems.set(ServiceProblems::FIRE, true);
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    assert_eq!(FORMAT_VERSION, 24);
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(
        l2.data
            .service_state
            .get(ids[0])
            .unwrap()
            .fire
            .unwrap()
            .station,
        Some(ids[1])
    );
}

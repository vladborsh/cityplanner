//! Format 53 (T-247): mail. The v52 fixture (made by the real v52 writer, a grid city with no
//! mail) loads with no mail waiting and no backlog, and the mail of a post office's town
//! round-trips.

use std::path::Path;

use cp_building::BuildingId;
use cp_content::ContentDb;
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v52_fixture_loads_without_mail_and_mail_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v52-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 52, "made by the v52 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data.mail, cp_service::MailState::default());
    assert!(l.data.buildings.iter().count() > 10, "the city is kept");
    assert!(l.data.citizens.citizen_count() > 0);

    let house = BuildingId::new(2, 0);
    l.data.mail.set_pile(house, 33);
    l.data.mail.unsorted = 1200;
    l.data.mail.sort_capacity = 800;
    l.data.mail.today.made = 70;
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    assert_eq!(FORMAT_VERSION, 55);
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.mail.pile(house), 33);
    assert_eq!(l2.data.mail.unsorted, 1200);
}

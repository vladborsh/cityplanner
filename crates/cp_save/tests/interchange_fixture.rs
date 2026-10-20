//! T-064: the interchange fixture (a ground street with two raised roads over it that cross each
//! other in a node, made by the current writer, T-275) loads, keeps its levels and round-trips.

use std::path::Path;

use cp_content::ContentDb;
use cp_net::{Elevation, NetGraph};
use cp_save::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_interchange_fixture_loads_with_its_levels_and_its_crossing_node() {
    let bytes =
        std::fs::read(Path::new(ROOT).join("../../fixtures/saves/interchange.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(
        h.format_version, FORMAT_VERSION,
        "made by the current writer"
    );
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let net: &NetGraph = &l.data.net;
    let level = |e: Elevation| net.segments().filter(|(_, s)| s.elevation == e).count();
    assert!(level(Elevation::Ground) >= 2, "the street");
    assert!(
        level(Elevation::Elevated) >= 6,
        "two decks of several pieces"
    );
    // The two decks meet in one node of four raised pieces.
    assert!(
        net.nodes().any(|(_, n)| n.segments.len() == 4
            && n.segments
                .iter()
                .all(|&s| net.segment(s).unwrap().elevation == Elevation::Elevated)),
        "the crossing node"
    );
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
}

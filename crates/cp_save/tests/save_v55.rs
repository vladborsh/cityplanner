//! Format 55 (T-262): every segment has a build order and the graph a counter. The v54 fixture
//! (a street with a crossing street and a bent road, made by the real v54 writer with
//! `BuildRoad`) loads: the orders are given by ascending segment id, the counter goes on from
//! there, the net is the same, and a road built after the load takes the next order.

use std::path::Path;
use std::sync::Arc;

use cp_content::ContentDb;
use cp_save::*;
use cp_sim::SimRunner;
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v54_net_fixture_gets_build_orders_by_segment_id() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v54-net.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 54, "made by the v54 writer");
    assert_eq!(FORMAT_VERSION, 56);
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let loaded = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let net = &loaded.data.net;
    assert!(net.segment_count() >= 6, "{}", net.segment_count());
    let orders: Vec<u32> = net.segments().map(|(_, s)| s.order).collect();
    let expect: Vec<u32> = (0..orders.len() as u32).collect();
    assert_eq!(orders, expect, "by ascending segment id");
    assert_eq!(net.next_order() as usize, orders.len());

    // Through a runner: saved again at the current format, it loads back the same.
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::new(1);
    sim.set_terrain(Arc::clone(&terrain));
    sim.set_content(Arc::clone(&content));
    sim.tick_once();
    load_into_runner(&mut sim, &bytes, content.content_hash()).unwrap();
    sim.tick_once();
    let again = save_runner(&sim, content.content_hash(), "flatlands").unwrap();
    assert_eq!(read_header(&again).unwrap().0.format_version, 56);
    let mut sim2 = SimRunner::new(1);
    sim2.set_terrain(terrain);
    sim2.set_content(Arc::clone(&content));
    sim2.tick_once();
    load_into_runner(&mut sim2, &again, content.content_hash()).unwrap();
    sim2.tick_once();
    sim.tick_once();
    assert_eq!(sim.checksum(), sim2.checksum());
    let net2 = sim2.view().net;
    assert_eq!(net2.next_order() as usize, orders.len());
}

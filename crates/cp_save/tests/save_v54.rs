//! Format 54 (BUG-073b): the inner side of a rounded corner is laid from its virtual corner,
//! and the winner of a seam no longer depends on the arena slot. No serialized type changed, so
//! the saved blocks are laid again on load (like v46). The v53 fixture (made by the real v53
//! writer: an L with a rounded corner, its cells following the arc) loads, and its first sync
//! lays every block again by the current rules (since T-265, layout v3, which dropped the
//! rounded-corner rule of v54: the blocks a fresh grid lays on the same net).

use std::path::Path;
use std::sync::Arc;

use cp_content::ContentDb;
use cp_save::*;
use cp_sim::SimRunner;
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v53_corner_fixture_is_laid_again_on_load() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v53-corner.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 53, "made by the v53 writer");
    assert_eq!(FORMAT_VERSION, 57);
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());

    // As saved: the bend has cells on both sides.
    let loaded = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    let bend_sides = |zones: &cp_zone::ZoneGrid, net: &cp_net::NetGraph| {
        let bend = net
            .segments()
            .find(|(_, s)| {
                let (a, b) = (s.curve.tangent(0.0), s.curve.tangent(1.0));
                a.normalize().dot(b.normalize()) < 0.9
            })
            .map(|(id, _)| id)
            .expect("the bend");
        zones.blocks().filter(|(_, b)| b.segment == bend).count()
    };
    assert_eq!(bend_sides(&loaded.data.zones, &loaded.data.net), 2);

    // Into a runner and one tick: the first sync lays every block again.
    let mut sim = SimRunner::new(1);
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(&content));
    sim.tick_once();
    load_into_runner(&mut sim, &bytes, content.content_hash()).unwrap();
    sim.tick_once();
    let view = sim.view();
    // Laid again by the current rules (T-265, layout v3: the bend's sides are frontages of their
    // own again): the blocks are the ones a fresh grid lays on the same net.
    let mut fresh = cp_zone::ZoneGrid::new();
    cp_zone::sync_grid(&mut fresh, &view.net, &content);
    let params = |z: &cp_zone::ZoneGrid| {
        let mut v: Vec<_> = z
            .blocks()
            .map(|(_, b)| {
                (
                    b.segment,
                    b.side == cp_zone::Side::Left,
                    b.s0.to_bits(),
                    b.cols,
                )
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(params(&view.zones), params(&fresh), "laid again on load");
    assert_eq!(bend_sides(&view.zones, &view.net), 2);
    assert!(
        view.zones.blocks().count() > 4,
        "the streets keep their cells"
    );
    // The saved again file is the current format and loads back the same.
    let again = save_runner(&sim, content.content_hash(), "flatlands").unwrap();
    assert_eq!(read_header(&again).unwrap().0.format_version, 57);
    let mut sim2 = SimRunner::new(1);
    sim2.set_terrain(Arc::new(
        Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap(),
    ));
    sim2.set_content(Arc::clone(&content));
    sim2.tick_once();
    load_into_runner(&mut sim2, &again, content.content_hash()).unwrap();
    sim2.tick_once();
    sim.tick_once();
    assert_eq!(sim.checksum(), sim2.checksum());
}

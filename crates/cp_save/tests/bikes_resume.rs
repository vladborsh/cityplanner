//! §23.6 item 6 (T-322a): a game saved with cyclists on their way loads with them (the
//! `bike` flag and the trip mode are saved) and runs on to the same checksum as the game that
//! went on.

use std::path::Path;
use std::sync::Arc;

use cp_content::ContentDb;
use cp_core::SimLimits;
use cp_save::{load_into_runner, save_runner};
use cp_sim::SimRunner;
use cp_terrain::Terrain;
use cp_traffic::Agents;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn runner(content: &Arc<ContentDb>, bytes: &[u8]) -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(content));
    load_into_runner(&mut sim, bytes, content.content_hash()).unwrap();
    sim
}

fn riding(sim: &SimRunner) -> usize {
    sim.world()
        .resource::<Agents>()
        .pedestrians()
        .filter(|(_, p)| p.bike)
        .count()
}

#[test]
fn cyclists_on_their_way_survive_a_save_and_ride_on_alike() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let fixture = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/m1-small.sav")).unwrap();
    let mut a = runner(&content, &fixture);
    for _ in 0..2 * cp_core::time::TICKS_PER_DAY {
        if riding(&a) >= 3 {
            break;
        }
        a.tick_once();
    }
    let n = riding(&a);
    assert!(n >= 3, "cyclists on their way: {n}");
    let bytes = save_runner(&a, content.content_hash(), "flatlands").unwrap();
    let mut b = runner(&content, &bytes);
    assert_eq!(riding(&b), n);
    assert_eq!(a.checksum(), b.checksum(), "state at load");
    for _ in 0..cp_core::time::TICKS_PER_DAY {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum(), "load + a day");
}

//! T-318a (§19.7 item 6): the contentment of the homes is state (migration reads it every tick
//! between the 256-tick passes), so a game saved in the middle of a cycle loads with the very
//! records the run that went on has, and the two stay identical through the next passes.

use std::path::Path;
use std::sync::Arc;

use cp_building::HomeContentment;
use cp_content::ContentDb;
use cp_core::SimLimits;
use cp_economy::CityContentment;
use cp_save::{load_into_runner, save_runner};
use cp_sim::SimRunner;
use cp_terrain::Terrain;

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

#[test]
fn a_game_saved_between_passes_keeps_the_contentment_and_runs_on_alike() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let fixture = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/m1-small.sav")).unwrap();
    let mut a = runner(&content, &fixture);
    // Past a pass and into the middle of the next cycle.
    for _ in 0..(256 + 100) {
        a.tick_once();
    }
    assert_ne!(a.tick().0 % 256, 0, "between two passes");
    let homes = a.world().resource::<HomeContentment>().len();
    assert!(homes > 50, "the town's homes have a contentment: {homes}");
    assert!(a.world().resource::<CityContentment>().homes > 0);
    let bytes = save_runner(&a, content.content_hash(), "flatlands").unwrap();
    let mut b = runner(&content, &bytes);
    assert!(
        a.world().resource::<HomeContentment>() == b.world().resource::<HomeContentment>(),
        "the homes right after the load"
    );
    assert_eq!(
        a.world().resource::<CityContentment>(),
        b.world().resource::<CityContentment>(),
        "the roll-ups right after the load"
    );
    assert_eq!(a.checksum(), b.checksum(), "state at load");
    for _ in 0..512 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum(), "load + 512 ticks");
}

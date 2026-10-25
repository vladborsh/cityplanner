//! §20.6 item 7 (T-319a): the service budgets are saved; a game saved with budgets off 100 %
//! in the middle of a 64-tick cycle loads with the same budgets and runs on to the same
//! checksum as the game that went on (coverage, pools and capacities read them alike).

use std::path::Path;
use std::sync::Arc;

use cp_content::{Budgets, ContentDb, ServiceKind};
use cp_core::SimLimits;
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
fn budgets_survive_a_save_mid_cycle_and_run_on_alike() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let fixture = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/m1-small.sav")).unwrap();
    let mut a = runner(&content, &fixture);
    let mut budgets = Budgets::default();
    for (kind, pct) in [
        (ServiceKind::Water, 60),
        (ServiceKind::Power, 140),
        (ServiceKind::Roads, 50),
        (ServiceKind::Health, 75),
        (ServiceKind::Garbage, 150),
    ] {
        budgets.set(kind, pct);
    }
    a.world_mut().insert_resource(budgets);
    // Into the middle of a 64-tick cycle: the pools and the coverage of the budgets are not
    // computed yet when the game is saved.
    while a.tick().0 % 64 != 30 {
        a.tick_once();
    }
    let bytes = save_runner(&a, content.content_hash(), "flatlands").unwrap();
    let mut b = runner(&content, &bytes);
    assert_eq!(*b.world().resource::<Budgets>(), budgets);
    assert_eq!(a.checksum(), b.checksum(), "state at load");
    for _ in 0..cp_core::time::TICKS_PER_DAY {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum(), "load + a day");
}

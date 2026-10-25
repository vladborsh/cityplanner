//! §21.6 item 5 (T-320a): the market is saved; a game saved in the middle of a day with a
//! shortage in flight and levels off 100 % loads with the same market and runs two more days to
//! the same checksum as the game that went on.

use std::path::Path;
use std::sync::Arc;

use cp_content::market::{MarketEvent, MarketEventKind};
use cp_content::{CargoKind, ContentDb, Market};
use cp_core::SimLimits;
use cp_core::time::TICKS_PER_DAY;
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
fn the_market_survives_a_save_mid_day_and_runs_on_alike() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let fixture = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/m1-small.sav")).unwrap();
    let mut a = runner(&content, &fixture);
    let day = (a.tick().0 / TICKS_PER_DAY) as u32;
    {
        let mut m = a.world_mut().resource_mut::<Market>();
        m.get_mut(CargoKind::Metals).event = Some(MarketEvent {
            kind: MarketEventKind::Shortage,
            until_day: day + 10,
        });
        m.get_mut(CargoKind::Logs).index = 140;
        m.get_mut(CargoKind::Goods).index = 70;
    }
    while a.tick().0 % TICKS_PER_DAY != TICKS_PER_DAY / 2 {
        a.tick_once();
    }
    let market = *a.world().resource::<Market>();
    let bytes = save_runner(&a, content.content_hash(), "flatlands").unwrap();
    let mut b = runner(&content, &bytes);
    assert_eq!(*b.world().resource::<Market>(), market);
    assert_eq!(a.checksum(), b.checksum(), "state at load");
    for _ in 0..2 * TICKS_PER_DAY {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum(), "load + two days");
    assert_ne!(
        *b.world().resource::<Market>(),
        market,
        "the days moved the market on"
    );
}

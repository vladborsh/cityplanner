//! T-324: the corruption state is saved whole: a game saved with evidence, an offer and a
//! captured station loads with the same state and runs on to the same checksum.

use std::path::Path;
use std::sync::Arc;

use cp_content::ContentDb;
use cp_core::SimLimits;
use cp_economy::{CorruptionState, DealTarget, Evidence, Offer};
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
fn the_corruption_state_survives_a_save_and_runs_on_alike() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let fixture = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/m1-small.sav")).unwrap();
    let mut a = runner(&content, &fixture);
    let station = a
        .world()
        .resource::<cp_building::BuildingGrid>()
        .iter()
        .next()
        .map(|(id, _)| id)
        .expect("a building");
    *a.world_mut().resource_mut::<CorruptionState>() = CorruptionState {
        corruption: 33.5,
        institutions: 12.25,
        trust: 41.0,
        started: true,
        evidence: vec![Evidence {
            deal: "kickback".into(),
            accepted_day: 3,
            weight: 2.0,
        }],
        inbox: vec![Offer {
            deal: "kickback".into(),
            target: Some(DealTarget::Building(station)),
            expires_day: 9,
        }],
        captured: vec![station],
        next_offer_day: 12,
        scandal_cooldown_until: 4,
    };
    a.tick_once();
    let bytes = save_runner(&a, content.content_hash(), "flatlands").unwrap();
    let mut b = runner(&content, &bytes);
    assert_eq!(
        a.world().resource::<CorruptionState>(),
        b.world().resource::<CorruptionState>()
    );
    assert_eq!(a.checksum(), b.checksum(), "state at load");
    for _ in 0..cp_core::time::TICKS_PER_DAY {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum(), "load + a day");
}

//! T-075 acceptance: the demo traffic enters the sim as a logged `DebugCommand`, so a run
//! with it is deterministic by checksum and replays from a save plus the log.

use std::path::Path;
use std::sync::Arc;

use cp_content::ContentDb;
use cp_core::CommandLog;
use cp_save::{load_into_runner, save_runner};
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_sim::{DebugCommand, SimCommand, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn env() -> (Arc<Terrain>, Arc<ContentDb>) {
    let root = Path::new(ROOT).join("../..");
    (
        Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap()),
        Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap()),
    )
}

fn town() -> SimRunner {
    let (terrain, content) = env();
    let mut sim = grid_city(400, 3, terrain, content, &GridCityConfig::default()).0;
    // The injected trips keep the mode they were asked for.
    sim.world_mut()
        .resource_mut::<cp_traffic::AgentSettings>()
        .mode_by_time = false;
    sim
}

#[test]
fn injected_traffic_is_deterministic_and_logged() {
    let (mut a, mut b) = (town(), town());
    a.top_up_traffic(300, 300);
    // The same command through the queue, at the same tick.
    let now = b.tick();
    b.push_command(
        SimCommand::Debug(DebugCommand::TopUpTraffic {
            cars: 300,
            peds: 300,
        }),
        now,
    );
    for _ in 0..200 {
        a.tick_once();
        b.tick_once();
    }
    let (cars, peds) = a.agent_counts();
    // Walks end at the junction setbacks (BUG-064), so many of the 300 have arrived by now.
    assert!(cars > 100 && peds > 50, "{cars} cars {peds} pedestrians");
    assert_eq!(a.checksum(), b.checksum());
    let log = a.world().resource::<CommandLog<SimCommand>>();
    assert!(log.entries().iter().any(|e| matches!(
        e.command,
        SimCommand::Debug(DebugCommand::TopUpTraffic { .. })
    )));
    // A full pool adds nothing: rejected, so not logged again.
    let n = a.world().resource::<CommandLog<SimCommand>>().len();
    a.top_up_traffic(1, 1);
    assert_eq!(a.world().resource::<CommandLog<SimCommand>>().len(), n);
}

#[test]
fn a_run_with_injected_traffic_resumes_from_a_save() {
    let (terrain, content) = env();
    let mut a = town();
    a.top_up_traffic(200, 200);
    for _ in 0..60 {
        a.tick_once();
    }
    // Pedestrians and cars are on their way (a pedestrian's resolved leg is a cache that is
    // not saved and refills after the load).
    let (cars, peds) = a.agent_counts();
    assert!(
        cars > 0 && peds > 0,
        "{cars} cars {peds} pedestrians at the save"
    );
    let bytes = save_runner(&a, content.content_hash(), "flatlands").unwrap();
    let mut b = SimRunner::new(1);
    b.world_mut()
        .resource_mut::<cp_traffic::AgentSettings>()
        .mode_by_time = false;
    b.set_terrain(terrain);
    b.set_content(Arc::clone(&content));
    load_into_runner(&mut b, &bytes, content.content_hash()).unwrap();
    assert_eq!(a.checksum(), b.checksum());
    assert_eq!(a.tick(), b.tick());
    for _ in 0..200 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum());
}

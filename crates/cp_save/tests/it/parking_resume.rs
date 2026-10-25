//! §18.3 (T-331b1): the parked cars and the reservations of the trips on the road are saved. A
//! game saved while a citizen walks from a parked car to the goal loads with the same state and
//! runs on to the same checksum as the game that went on.

use std::path::Path;
use std::sync::Arc;

use cp_citizen::{TripBook, TripState};
use cp_content::ContentDb;
use cp_core::SimLimits;
use cp_save::{load_into_runner, save_runner};
use cp_sim::SimRunner;
use cp_terrain::Terrain;
use cp_traffic::{Agents, ParkingIndex};

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

/// Trips whose car stands and whose citizen is on foot to the goal.
fn walking_from_a_car(sim: &SimRunner) -> usize {
    sim.world()
        .resource::<TripBook>()
        .iter()
        .filter(|(_, t)| t.park.parked && t.park.place.is_some())
        .filter(|(_, t)| !matches!(t.state, TripState::Scheduled { .. }))
        .count()
}

#[test]
fn a_save_mid_walk_from_the_car_runs_on_alike() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let fixture =
        std::fs::read(Path::new(ROOT).join("../../fixtures/saves/m1-traffic.sav")).unwrap();
    let mut a = runner(&content, &fixture);
    // Every car trip takes a car, so cars park soon.
    let mut ticks = 0;
    while walking_from_a_car(&a) == 0 {
        a.tick_once();
        ticks += 1;
        assert!(
            ticks < 4 * cp_core::time::TICKS_PER_DAY,
            "no car ever parked"
        );
    }
    let parked = a.world().resource::<Agents>().parked.len();
    assert!(parked > 0);
    let bytes = save_runner(&a, content.content_hash(), "flatlands").unwrap();
    let mut b = runner(&content, &bytes);
    assert_eq!(a.checksum(), b.checksum(), "state at load");
    assert_eq!(b.world().resource::<Agents>().parked.len(), parked);
    for _ in 0..cp_core::time::TICKS_PER_DAY {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum(), "load + a day");
    // The index is derived: counted again from the parked cars and the reservations.
    let (ia, ib) = (
        a.world().resource::<ParkingIndex>(),
        b.world().resource::<ParkingIndex>(),
    );
    assert_eq!(ia.kerb_total(), ib.kerb_total());
    assert_eq!(ia.building_total(), ib.building_total());
}

//! BUG-051: a need whose remedy is locked by a milestone is not generated: no sick, no bodies,
//! no garbage, no crime, no fires, and no problem bits, until the service opens.

use std::path::Path;
use std::sync::Arc;

use cp_building::BuildingGrid;
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ServiceKind, ServiceUnlocks};
use cp_core::time::TICKS_PER_DAY;
use cp_service::ServiceState;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_sim::{Progress, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn city() -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let (sim, report) = grid_city(120, 1, terrain, content, &GridCityConfig::default());
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    sim
}

/// Runs `n` ticks holding the progress at `p` (the grid city has more than 500 people, so the
/// progress pass would raise it to the second milestone by itself).
fn run(sim: &mut SimRunner, n: u64, p: &Progress) {
    for _ in 0..n {
        sim.tick_once();
        set_progress(sim, p.clone());
    }
}

fn set_progress(sim: &mut SimRunner, p: Progress) {
    sim.world_mut().insert_resource(p);
    cp_sim::progress::refresh_unlocks(sim.world_mut());
}

/// Everything the services count as a need, summed over the city.
struct Needs {
    sick: usize,
    dead: usize,
    garbage: u64,
    crime: u64,
    fires: usize,
    problems: usize,
}

fn needs(sim: &SimRunner) -> Needs {
    let w = sim.world();
    let grid = w.resource::<CitizenGrid>();
    let state = w.resource::<ServiceState>();
    let mut n = Needs {
        sick: grid
            .citizens()
            .filter(|(_, c)| c.sick_since.is_some())
            .count(),
        dead: grid.dead.values().map(|&d| usize::from(d)).sum(),
        garbage: 0,
        crime: 0,
        fires: 0,
        problems: 0,
    };
    for (id, _) in w.resource::<BuildingGrid>().iter() {
        if let Some(s) = state.get(id) {
            n.garbage += u64::from(s.garbage);
            n.crime += u64::from(s.crime);
            n.fires += usize::from(s.fire.is_some());
            n.problems += usize::from(!s.problems.is_empty());
        }
    }
    n
}

#[test]
fn nothing_is_needed_while_the_remedy_is_locked_and_it_starts_from_zero_after() {
    let mut sim = city();
    set_progress(&mut sim, Progress::at(0));
    let u = *sim.world().resource::<ServiceUnlocks>();
    for k in [
        ServiceKind::Garbage,
        ServiceKind::Health,
        ServiceKind::Deathcare,
        ServiceKind::Fire,
        ServiceKind::Police,
    ] {
        assert!(!u.open(k), "{k:?} is locked at the first milestone");
    }
    // Several days, sampled every quarter day: not one need anywhere.
    for _ in 0..8 * 4 {
        run(&mut sim, TICKS_PER_DAY / 4, &Progress::at(0));
        let n = needs(&sim);
        assert_eq!(
            (n.sick, n.dead, n.garbage, n.crime, n.fires, n.problems),
            (0, 0, 0, 0, 0, 0)
        );
    }

    // The second milestone opens health, garbage, fire and police: the needs begin.
    set_progress(&mut sim, Progress::at(1));
    let u = *sim.world().resource::<ServiceUnlocks>();
    assert!(u.open(ServiceKind::Garbage) && u.open(ServiceKind::Health));
    assert!(u.open(ServiceKind::Police) && u.open(ServiceKind::Fire));
    assert!(
        !u.open(ServiceKind::Deathcare),
        "cemeteries come at the third"
    );
    let mut sick_seen = false;
    let mut garbage_seen = false;
    for _ in 0..20 * 4 {
        run(&mut sim, TICKS_PER_DAY / 4, &Progress::at(1));
        let n = needs(&sim);
        sick_seen |= n.sick > 0;
        garbage_seen |= n.garbage > 0;
        assert_eq!(n.dead, 0, "no bodies wait while deathcare is locked");
    }
    assert!(garbage_seen, "garbage starts at once");
    assert!(sick_seen, "people fall ill once health is open");
}

#[test]
fn a_world_with_everything_open_is_unchanged() {
    // Progress::ALL (worlds built in code, old saves): no kind is locked.
    let mut sim = city();
    assert_eq!(
        *sim.world().resource::<ServiceUnlocks>(),
        ServiceUnlocks::default()
    );
    set_progress(&mut sim, Progress::ALL);
    assert_eq!(
        *sim.world().resource::<ServiceUnlocks>(),
        ServiceUnlocks::default()
    );
}

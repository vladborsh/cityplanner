//! T-247: mail in a real grid city. Locked (before the Town milestone) it makes no mail and no
//! problem; open, the city's post offices collect it with mail vans, sort it, and the city has
//! (almost) no MAIL problem; without them the problems come; the same city twice is the same.

use std::path::Path;
use std::sync::Arc;

use cp_building::BuildingGrid;
use cp_content::{ContentDb, ServiceKind, ServiceUnlocks, VehicleKind};
use cp_core::time::TICKS_PER_DAY;
use cp_service::{MailState, ServiceProblems, ServiceState};
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_sim::{Progress, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn city() -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let (sim, report) = grid_city(2500, 1, terrain, content, &GridCityConfig::default());
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    sim
}

/// Holds the progress at milestone `m` (the pass would raise it with the city's population).
fn run(sim: &mut SimRunner, ticks: u64, m: u8) {
    for _ in 0..ticks {
        sim.tick_once();
        sim.world_mut().insert_resource(Progress::at(m));
        cp_sim::progress::refresh_unlocks(sim.world_mut());
    }
}

fn mail_problems(sim: &SimRunner) -> usize {
    let state = sim.world().resource::<ServiceState>();
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(id, _)| state.problems_of(*id).has(ServiceProblems::MAIL))
        .count()
}

fn post_offices(sim: &SimRunner) -> usize {
    let db = &sim.world().resource::<cp_content::SimContent>().0;
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| {
            db.building(b.def)
                .service
                .as_deref()
                .and_then(|s| db.service_id(s))
                .is_some_and(|s| db.service(s).kind == ServiceKind::Post)
        })
        .count()
}

#[test]
fn no_mail_while_the_post_office_is_locked_and_vans_collect_it_after() {
    let mut sim = city();
    assert!(
        post_offices(&sim) >= 1,
        "the scripted city has a post office"
    );
    run(&mut sim, 3 * TICKS_PER_DAY, 1);
    assert!(
        !sim.world()
            .resource::<ServiceUnlocks>()
            .open(ServiceKind::Post)
    );
    let m = sim.world().resource::<MailState>();
    assert_eq!((m.piles().count(), m.unsorted, m.today.made), (0, 0, 0));
    assert_eq!(mail_problems(&sim), 0);

    // Town: the post office opens; the mail starts and is collected by vans.
    let mut vans_seen = false;
    for _ in 0..8 * 4 {
        run(&mut sim, TICKS_PER_DAY / 4, 2);
        let runs = sim.world().resource::<cp_traffic::ServiceRuns>();
        vans_seen |= runs.iter().any(|(_, r)| r.kind == VehicleKind::Mail as u8);
    }
    assert!(vans_seen, "mail vans were sent");
    let m = sim.world().resource::<MailState>();
    assert!(m.yesterday.made > 100, "{:?}", m.yesterday);
    assert!(
        m.yesterday.collected * 10 >= m.yesterday.made * 8,
        "most of the mail is collected: {:?}",
        m.yesterday
    );
    assert!(m.sort_capacity > 0);
    assert!(
        !m.backlog_full(
            sim.world()
                .resource::<cp_content::SimContent>()
                .0
                .balance()
                .mail_backlog_days
        )
    );
    let buildings = sim.world().resource::<BuildingGrid>().iter().count();
    assert!(
        mail_problems(&sim) * 20 <= buildings,
        "{} of {buildings} buildings have the MAIL problem",
        mail_problems(&sim)
    );
}

#[test]
fn without_post_offices_the_mail_piles_into_problems() {
    let mut sim = city();
    let posts: Vec<_> = {
        let db = &sim.world().resource::<cp_content::SimContent>().0;
        sim.world()
            .resource::<BuildingGrid>()
            .iter()
            .filter(|(_, b)| {
                db.building(b.def)
                    .service
                    .as_deref()
                    .and_then(|s| db.service_id(s))
                    .is_some_and(|s| db.service(s).kind == ServiceKind::Post)
            })
            .map(|(id, _)| id)
            .collect()
    };
    for id in posts {
        sim.world_mut().resource_mut::<BuildingGrid>().remove(id);
    }
    assert_eq!(post_offices(&sim), 0);
    run(&mut sim, 12 * TICKS_PER_DAY, 2);
    assert!(mail_problems(&sim) > 20, "{}", mail_problems(&sim));
}

#[test]
fn the_same_city_twice_has_the_same_mail() {
    let go = || {
        let mut sim = city();
        run(&mut sim, 4 * TICKS_PER_DAY, 2);
        (sim.checksum(), sim.world().resource::<MailState>().clone())
    };
    assert_eq!(go(), go());
}

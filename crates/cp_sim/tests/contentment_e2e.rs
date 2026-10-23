//! T-318a (§19.7 items 1, 2, 6): the contentment pass on a real city. Every active home gets a
//! record and nothing else does; the roll-ups add up; a home misses a service only once the
//! milestone has opened it; two runs are identical.

use std::path::Path;
use std::sync::Arc;

use cp_building::{BuildingGrid, BuildingState, Complaint, HomeContentment};
use cp_content::{ContentDb, ZoneType};
use cp_economy::CityContentment;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_sim::{Progress, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn city() -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let (sim, report) = grid_city(400, 1, terrain, content, &GridCityConfig::default());
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    sim
}

/// Runs `n` ticks with the progress held at `p` (the progress pass would raise it).
fn run(sim: &mut SimRunner, n: u64, p: &Progress) {
    for _ in 0..n {
        sim.tick_once();
        sim.world_mut().insert_resource(p.clone());
        cp_sim::progress::refresh_unlocks(sim.world_mut());
    }
}

fn at(reached: u8) -> Progress {
    Progress {
        reached,
        ..Progress::ALL
    }
}

const SERVICE_COMPLAINTS: [Complaint; 4] = [
    Complaint::NoHealth,
    Complaint::NoPolice,
    Complaint::NoFire,
    Complaint::NoSchool,
];

fn active_homes(sim: &SimRunner) -> usize {
    let db = sim.world().resource::<cp_content::SimContent>().0.clone();
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| {
            b.state == BuildingState::Active
                && matches!(
                    db.building(b.def).zone,
                    ZoneType::ResLow | ZoneType::ResHigh
                )
        })
        .count()
}

#[test]
fn every_active_home_is_scored_and_the_roll_ups_add_up() {
    let mut sim = city();
    run(&mut sim, 2 * cp_economy::CITY_PERIOD, &Progress::ALL);
    // Right after a pass, before anything else changed the buildings.
    while !sim.tick().0.is_multiple_of(cp_economy::CITY_PERIOD) {
        run(&mut sim, 1, &Progress::ALL);
    }
    let w = sim.world();
    let homes = w.resource::<HomeContentment>();
    let city = w.resource::<CityContentment>();
    assert_eq!(homes.len(), active_homes(&sim));
    assert_eq!(city.homes as usize, homes.len());
    assert_eq!(
        city.districts.iter().map(|d| d.homes).sum::<u32>(),
        city.homes
    );
    assert!(city.residents > 0);
    for (id, slot) in homes.homes() {
        let b = w.resource::<BuildingGrid>().get(id).expect("a building");
        assert_eq!(b.state, BuildingState::Active);
        assert!(slot.index <= 100);
        // Complaints first, empty places last, no repeats.
        let named: Vec<_> = slot
            .complaints
            .iter()
            .take_while(|&&c| c != Complaint::None)
            .collect();
        assert!(
            slot.complaints[named.len()..]
                .iter()
                .all(|&c| c == Complaint::None)
        );
        let mut uniq = named.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), named.len(), "{:?}", slot.complaints);
    }
    let named: u32 = city.complaints.iter().map(|&(_, n)| n).sum();
    let counted: usize = homes
        .homes()
        .map(|(_, s)| {
            s.complaints
                .iter()
                .filter(|&&c| c != Complaint::None)
                .count()
        })
        .sum();
    assert_eq!(named as usize, counted);
}

#[test]
fn homes_miss_a_service_only_once_its_milestone_opened_it() {
    let mut sim = city();
    // No clinic, police, fire station or school anywhere.
    let db = sim.world().resource::<cp_content::SimContent>().0.clone();
    let services: Vec<_> = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| db.building(b.def).service.is_some())
        .filter(|(_, b)| {
            let def = db.building(b.def);
            let kind = def
                .service
                .as_deref()
                .and_then(|s| db.service_id(s))
                .map(|s| db.service(s).kind);
            matches!(
                kind,
                Some(
                    cp_content::ServiceKind::Health
                        | cp_content::ServiceKind::Police
                        | cp_content::ServiceKind::Fire
                        | cp_content::ServiceKind::Education
                )
            )
        })
        .map(|(id, _)| id)
        .collect();
    assert!(!services.is_empty(), "the grid city places services");
    for id in services {
        sim.world_mut().resource_mut::<BuildingGrid>().remove(id);
    }
    // Hamlet: no service expected yet.
    run(&mut sim, 3 * cp_economy::CITY_PERIOD, &at(0));
    let city = sim.world().resource::<CityContentment>().clone();
    assert_eq!(city.factors[cp_economy::contentment::F_SERVICES], 100);
    for c in SERVICE_COMPLAINTS {
        assert!(
            city.complaints.iter().all(|&(k, _)| k != c),
            "{c:?} at Hamlet: {:?}",
            city.complaints
        );
    }
    // Everything open: the city has none of these buildings any more, so its homes miss them.
    run(&mut sim, 3 * cp_economy::CITY_PERIOD, &Progress::ALL);
    let city = sim.world().resource::<CityContentment>().clone();
    assert!(city.factors[cp_economy::contentment::F_SERVICES] < 100);
    assert!(
        city.complaints
            .iter()
            .any(|&(k, n)| SERVICE_COMPLAINTS.contains(&k) && n > 0),
        "{:?}",
        city.complaints
    );
}

#[test]
fn two_runs_score_the_same() {
    let go = || {
        let mut sim = city();
        run(&mut sim, 3 * cp_economy::CITY_PERIOD, &Progress::ALL);
        (
            sim.world().resource::<HomeContentment>().clone(),
            sim.world().resource::<CityContentment>().clone(),
            sim.checksum(),
        )
    };
    assert_eq!(go(), go());
}

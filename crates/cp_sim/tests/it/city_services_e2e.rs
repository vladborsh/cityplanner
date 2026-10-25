//! T-232: supply against demand per sector, read from a small headless city.

use std::path::Path;
use std::sync::Arc;

use cp_building::BuildingGrid;
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ServiceKind, ServiceUnlocks};
use cp_core::Tick;
use cp_sim::city_services::{BalanceInputs, Sector, city_services};
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn city() -> (cp_sim::SimRunner, Arc<ContentDb>) {
    let root = Path::new(ROOT).join("../..");
    let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
    let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
    let (mut sim, report) = grid_city(
        200,
        3,
        terrain,
        Arc::clone(&content),
        &GridCityConfig::default(),
    );
    assert!(report.rejected.is_empty(), "{report:?}");
    for _ in 0..130 {
        sim.tick_once();
    }
    (sim, content)
}

#[test]
fn a_city_with_sources_shows_a_surplus_and_a_city_without_a_hospital_a_shortage() {
    let (mut sim, content) = city();
    let view = sim.view();
    let rows = view.city_services().to_vec();
    assert_eq!(rows.len(), Sector::ALL.len());
    assert!(rows.iter().zip(Sector::ALL).all(|(r, s)| r.sector == s));
    let get = |rows: &[cp_sim::city_services::SectorBalance], s: Sector| {
        *rows.iter().find(|r| r.sector == s).unwrap()
    };
    // The towers and turbines of the scenario are sized above the town's needs.
    let water = get(&rows, Sector::Water);
    assert!(
        water.demand > 0.0 && water.supply >= water.demand,
        "{water:?}"
    );
    assert!(water.balance() >= 0.0);
    // Garbage: the landfill takes more than the town makes (16 % more since T-300: the service
    // buildings no longer pollute, so the homes stand higher and make more).
    let garbage = get(&rows, Sector::Garbage);
    assert!(
        garbage.demand > 0.0 && garbage.balance() > 0.1,
        "{garbage:?}"
    );

    // Make everybody sick and take the clinics and hospitals away: shortage.
    let ids: Vec<_> = sim
        .world()
        .resource::<CitizenGrid>()
        .citizens()
        .map(|(id, _)| id)
        .collect();
    let health: Vec<_> = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| {
            content
                .building(b.def)
                .service
                .as_deref()
                .and_then(|s| content.service_id(s))
                .is_some_and(|s| content.service(s).kind == ServiceKind::Health)
        })
        .map(|(id, _)| id)
        .collect();
    assert!(!health.is_empty(), "the scenario builds a clinic");
    for id in health {
        sim.world_mut().resource_mut::<BuildingGrid>().remove(id);
    }
    for id in ids {
        sim.world_mut()
            .resource_mut::<CitizenGrid>()
            .set_sick(id, Some(Tick(1)));
    }
    for _ in 0..64 {
        sim.tick_once();
    }
    let h = get(sim.view().city_services(), Sector::Health);
    assert_eq!(h.supply, 0.0, "{h:?}");
    assert!(h.demand > 100.0 && h.balance() <= -1.0, "{h:?}");
}

#[test]
fn a_sector_whose_service_is_not_open_is_locked() {
    let (sim, content) = city();
    let mut unlocks = ServiceUnlocks::default();
    unlocks.set(ServiceKind::Education, false);
    let rows = city_services(&BalanceInputs {
        db: &content,
        buildings: sim.world().resource::<BuildingGrid>(),
        citizens: sim.world().resource::<CitizenGrid>(),
        coverage: sim.world().resource::<cp_service::ServiceCoverage>(),
        unlocks,
        utilities: [(0.0, 0.0); 3],
        died_today: 0,
        budgets: Default::default(),
    });
    for r in &rows {
        let edu = matches!(
            r.sector,
            Sector::Elementary | Sector::HighSchool | Sector::University
        );
        assert_eq!(r.locked, edu, "{:?}", r.sector);
    }
}

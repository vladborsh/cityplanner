//! BUG-040: the early town (~250 residents, water, power and sewage only) is about break-even
//! per day, money is booked every game hour, and the flows are there to show.

use std::path::Path;
use std::sync::Arc;

use cp_building::{BuildingCommand, BuildingGrid};
use cp_content::{ContentDb, ServiceKind};
use cp_core::time::TICKS_PER_DAY;
use cp_economy::{EconomyState, MoneyFlow};
use cp_sim::scenario::{GridCityConfig, grid_city, population};
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// A grid city of about 250 people with only the services a starting town has: the others are
/// demolished, so the day's money is the early game's.
fn early_town() -> SimRunner {
    let root = Path::new(ROOT).join("../..");
    let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
    let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
    let (mut sim, report) = grid_city(250, 1, terrain, content.clone(), &GridCityConfig::default());
    assert!(report.rejected.is_empty(), "{:?}", report.rejected);
    let keep = [ServiceKind::Water, ServiceKind::Power, ServiceKind::Sewage];
    let drop: Vec<_> = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| {
            let d = content.building(b.def);
            d.service
                .as_deref()
                .and_then(|s| content.service_id(s))
                .is_some_and(|s| !keep.contains(&content.service(s).kind))
        })
        .map(|(id, _)| id)
        .collect();
    for building in drop {
        let tick = sim.tick();
        sim.push_command(
            SimCommand::Building(BuildingCommand::Demolish { building }),
            tick,
        );
    }
    // Growth stops where it is; the buildings under construction finish within a day.
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(0.0);
    for _ in 0..TICKS_PER_DAY {
        sim.tick_once();
    }
    sim
}

#[test]
fn money_is_booked_every_hour_and_the_early_town_is_about_break_even_per_day() {
    let mut sim = early_town();
    let start = sim.world().resource::<cp_core::Treasury>().balance;
    let first = cp_economy::game_hour(sim.tick().0);
    for _ in 0..TICKS_PER_DAY {
        sim.tick_once();
    }
    let flow = sim.world().resource::<MoneyFlow>();
    let last: Vec<_> = flow.hours.iter().rev().take(24).collect();
    assert!(
        last.windows(2).all(|w| w[0].0 == w[1].0 + 1),
        "one booking per game hour"
    );
    assert!(last.iter().all(|(_, h)| h.tax() > cp_core::Cost::ZERO));
    let day = flow
        .last_day(sim.world().resource::<EconomyState>())
        .unwrap();
    let booked: cp_core::Cost = flow
        .hours
        .iter()
        .filter(|(h, _)| *h > first)
        .map(|(_, f)| f.net())
        .sum();
    assert_eq!(
        sim.world().resource::<cp_core::Treasury>().balance,
        start + booked,
        "the treasury moved by exactly what the flow shows"
    );
    let pop = population(&sim);
    eprintln!("pop {pop} day {day:?}");
    assert!((150..=700).contains(&pop), "{pop} residents");
    assert!(
        day.net().0 * 20 >= -day.income().0,
        "balance per day {} against income {}",
        day.net().0,
        day.income().0
    );
}

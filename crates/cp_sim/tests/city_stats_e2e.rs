//! T-084 acceptance: `CityStats` matches a direct count over the arenas on a `grid_city`, the
//! history gets exactly 24 samples per game day, and the summary is outside the checksum and
//! never read by the simulation.

use std::path::Path;
use std::sync::Arc;

use cp_building::{BuildingFlags, BuildingGrid, BuildingState};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ZoneType};
use cp_core::time::TICKS_PER_DAY;
use cp_net::NetGraph;
use cp_sim::SimRunner;
use cp_sim::scenario::{GridCityConfig, grid_city};
use cp_sim::stats::{CityStats, STATS_PERIOD, ZONES};
use cp_terrain::Terrain;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn city(seed: u64) -> SimRunner {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let terrain = Arc::new(Terrain::load(&root.join("assets/maps/flatlands")).unwrap());
    let content = Arc::new(ContentDb::load_dir(root.join("assets/content")).unwrap());
    let (sim, report) = grid_city(1500, seed, terrain, content, &GridCityConfig::default());
    assert!(report.reached, "{report:?}");
    sim
}

fn to_boundary(sim: &mut SimRunner) {
    // The recompute runs in slices on ticks 0..=4 of each 64-tick period and publishes at the
    // fifth: stop right after it.
    while sim.tick().0 % STATS_PERIOD != 5 {
        sim.tick_once();
    }
}

#[test]
fn counters_match_a_direct_count_over_the_arenas() {
    let mut sim = city(3);
    to_boundary(&mut sim);
    let w = sim.world();
    let s = w.resource::<CityStats>();
    assert!(s.generation > 0);
    let citizens = w.resource::<CitizenGrid>();
    assert_eq!(s.population as usize, citizens.citizen_count());
    assert_eq!(s.households as usize, citizens.household_count());
    assert_eq!(s.adults as usize, citizens.adults());
    assert_eq!(s.employed as usize, citizens.employed());
    assert_eq!(s.unemployed as usize, citizens.unemployed());
    assert!(s.population >= 1500);

    // Buildings by state and by flag.
    let b = w.resource::<BuildingGrid>();
    let count =
        |f: &dyn Fn(&cp_building::Building) -> bool| b.iter().filter(|(_, x)| f(x)).count() as u32;
    assert_eq!(s.buildings.total as usize, b.len());
    assert_eq!(
        s.buildings.constructing,
        count(&|x| x.state == BuildingState::UnderConstruction)
    );
    assert_eq!(
        s.buildings.active,
        count(&|x| x.state == BuildingState::Active)
    );
    assert_eq!(
        s.buildings.abandoned,
        count(&|x| x.state == BuildingState::Abandoned)
    );
    // flatlands has no water for a pump: every building that needs water lacks it.
    let no_water = count(&|x| x.flags.has(BuildingFlags::NO_WATER));
    assert!(
        no_water > 0,
        "the city has no pump, so somebody lacks water"
    );
    assert_eq!(s.buildings.no_water, no_water);
    assert_eq!(
        s.buildings.no_power,
        count(&|x| x.flags.has(BuildingFlags::NO_POWER))
    );
    assert_eq!(
        s.buildings.no_sewage,
        count(&|x| x.flags.has(BuildingFlags::NO_SEWAGE))
    );
    assert_eq!(
        s.buildings.no_road,
        count(&|x| x.flags.has(BuildingFlags::NO_ROAD))
    );
    assert_eq!(
        s.utilities[0].unsupplied,
        count(&|x| x.state == BuildingState::Active && x.flags.has(BuildingFlags::NO_WATER))
    );
    assert!(
        s.utilities[2].sources > 0 && s.utilities[2].capacity > 0.0,
        "wind turbines"
    );

    // Zone cells by painted type, valid and occupied.
    let zones = w.resource::<ZoneGrid>();
    for (k, z) in ZONES.iter().enumerate() {
        let (mut painted, mut valid, mut occupied) = (0, 0, 0);
        for (id, block) in zones.blocks() {
            for i in 0..block.zones.len() {
                if block.zones[i] == *z {
                    painted += 1;
                    valid += u32::from(block.valid[i]);
                    occupied += u32::from(zones.is_occupied(ZoneCellRef {
                        block: id,
                        cell: i as u16,
                    }));
                }
            }
        }
        let zs = &s.zones[k];
        assert_eq!(
            (zs.painted, zs.valid, zs.occupied),
            (painted, valid, occupied),
            "{z:?}"
        );
    }
    assert!(s.zones.iter().any(|z| z.painted > 0 && z.occupied > 0));
    let res_buildings: u32 = (0..2)
        .map(|k| s.zones[k].by_level.iter().sum::<u32>())
        .sum();
    assert!(res_buildings > 0);
    let homes: u32 = s.zones.iter().map(|z| z.homes).sum();
    let free: u32 = s.zones.iter().map(|z| z.homes_free).sum();
    assert!(free <= homes && homes > 0);

    // Roads by type.
    let net = w.resource::<NetGraph>();
    assert_eq!(
        s.roads.iter().map(|g| g.segments as usize).sum::<usize>(),
        net.segment_count()
    );
    for g in &s.roads {
        let direct = net.segments().filter(|(_, x)| x.road == g.road).count();
        assert_eq!(g.segments as usize, direct);
        assert!(g.meters > 0.0 && g.upkeep_week.0 > 0);
    }
    assert!(s.roads.len() >= 2, "basic and medium roads");
    let _ = ZoneType::None;
}

#[test]
fn history_gets_exactly_24_samples_per_game_day() {
    let mut sim = city(3);
    to_boundary(&mut sim);
    // Start of a fresh game day.
    while !sim.tick().0.is_multiple_of(TICKS_PER_DAY) {
        sim.tick_once();
    }
    sim.tick_once();
    let before = sim.world().resource::<CityStats>().history.len();
    for _ in 0..TICKS_PER_DAY {
        sim.tick_once();
    }
    let s = sim.world().resource::<CityStats>();
    assert_eq!(
        s.history.len() - before,
        24,
        "{before} -> {}",
        s.history.len()
    );
    // The samples are an hour apart.
    let ticks: Vec<u64> = s.history.iter().map(|h| h.tick).collect();
    assert!(ticks.windows(2).all(|w| w[1] > w[0]));
    assert!(s.history.back().unwrap().population as usize >= 1500);
}

#[test]
fn the_summary_is_outside_the_checksum_and_not_read_by_the_sim() {
    let (mut a, mut b) = (city(4), city(4));
    for t in 0..400 {
        a.tick_once();
        b.tick_once();
        // Run B trashes the summary every tick: nothing in the sim may notice.
        if t % 7 == 0 {
            let mut s = b.world_mut().resource_mut::<CityStats>();
            s.population = 42;
            s.zones[0].painted = 7;
            s.history.clear();
        }
    }
    assert_eq!(a.checksum(), b.checksum());
}

#[test]
fn the_view_shares_the_summary_until_it_is_recomputed() {
    let mut sim = city(3);
    to_boundary(&mut sim);
    let (v1, v2) = (sim.view(), sim.view());
    assert!(Arc::ptr_eq(&v1.city, &v2.city));
    sim.tick_once();
    assert!(Arc::ptr_eq(&v1.city, &sim.view().city));
    for _ in 0..STATS_PERIOD {
        sim.tick_once();
    }
    let v3 = sim.view();
    assert!(!Arc::ptr_eq(&v1.city, &v3.city));
    assert!(v3.city.generation > v1.city.generation);
}

//! T-041 scenario tests.
use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingGrid, Demand};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_terrain::TileOwnership;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn lattice(lines: usize, kinds: &[ZoneType]) -> SimRunner {
    let content = content();
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(7);
    sim.set_content(Arc::clone(&content));
    sim.world_mut().insert_resource(TileOwnership::only((2, 2)));
    // No water or power lines in these scenarios: growth, not decay, is under test.
    *sim.world_mut().resource_mut::<cp_economy::Decay>() = cp_economy::Decay::disabled();
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(2000.0 + x, 0.0, 2000.0 + z);
        for line in 0..lines {
            let o = line as f32 * 90.0;
            for piece in 0..lines - 1 {
                let a = piece as f32 * 90.0;
                for (from, to) in [(p(a, o), p(a + 90.0, o)), (p(o, a), p(o, a + 90.0))] {
                    g.add_segment(&def, road, CubicBezier3::line(from, to), Elevation::Ground)
                        .unwrap();
                }
            }
        }
    }
    sim.tick_once();
    {
        let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
        let ids: Vec<_> = grid.blocks().map(|(id, b)| (id, b.cell_count())).collect();
        for (n, (id, cells)) in ids.into_iter().enumerate() {
            for cell in 0..cells {
                grid.set_zone(
                    ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    },
                    kinds[n % kinds.len()],
                );
            }
        }
    }
    sim
}

use ZoneType::*;

const MIX: [ZoneType; 8] = [
    ResHigh, ResLow, ComHigh, Industrial, ResHigh, ResHigh, Office, ComLow,
];

/// Runs whole days until `residents` live in the city; returns the days it took.
fn grow_to(sim: &mut SimRunner, residents: usize, max_days: u64) -> u64 {
    for day in 1..=max_days {
        for _ in 0..TICKS_PER_DAY {
            sim.tick_once();
        }
        if sim.world().resource::<CitizenGrid>().citizen_count() >= residents {
            return day;
        }
    }
    panic!(
        "only {} residents after {max_days} days (demand {:?})",
        sim.world().resource::<CitizenGrid>().citizen_count(),
        sim.world().resource::<Demand>()
    );
}

/// Nobody paints, builds or moves anyone: zoning a street lattice is the only input, and the
/// demand decides what spawns where and who moves in.
#[test]
fn a_zoned_district_grows_to_a_thousand_residents_unaided() {
    let mut sim = lattice(8, &MIX);
    let days = grow_to(&mut sim, 1000, 10);
    assert!(days <= 10);
    let content = content();
    let b = sim.world().resource::<BuildingGrid>();
    let zones: std::collections::BTreeSet<_> = b
        .iter()
        .map(|(_, bd)| content.building(bd.def).zone)
        .collect();
    for z in [ResHigh, ComHigh, Industrial] {
        assert!(zones.contains(&z), "{z:?} was never built: {zones:?}");
    }
    // Work follows: a few days on (jobs are built after the homes), most adults have it.
    for _ in 0..(6 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    let c = sim.world().resource::<CitizenGrid>();
    assert!(
        c.employed() * 2 > c.adults(),
        "{} of {}",
        c.employed(),
        c.adults()
    );
}

/// BUG-076: where the shop / industry ratios are met but ~290 adults stay jobless (R negative),
/// C or I must still ask for jobs. Before the fix a 1 550-resident district sat at R -20,
/// C -8, I -80 for weeks (growth stalled); now I stays positive while the jobless are counted.
#[test]
fn the_jobless_make_commercial_or_industrial_demand_when_residential_is_empty() {
    let mut sim = lattice(6, &[ResLow, ResLow, ResHigh, ComLow, Industrial, ResLow]);
    for day in 1..=12u64 {
        for _ in 0..TICKS_PER_DAY {
            sim.tick_once();
        }
        let c = sim.world().resource::<CitizenGrid>();
        let jobless = c.adults() - c.employed();
        let d = *sim.world().resource::<Demand>();
        if day >= 4 && d.r <= 0.0 && jobless > 150 {
            assert!(
                d.c > 0.0 || d.i > 0.0,
                "day {day}: {jobless} jobless but demand {d:?}"
            );
        }
    }
}

/// BUG-076b: a district whose industrial leftovers are one column wide has demand for industry
/// (I > 0) but no cell a building can cover; the stat says so.
#[test]
fn leftover_slivers_count_as_unfit_land_while_industry_is_wanted() {
    let mut sim = lattice(6, &[ResLow, ResLow, ResHigh, ComLow, Industrial, ResLow]);
    for _ in 0..10 * TICKS_PER_DAY {
        sim.tick_once();
    }
    let view = sim.view();
    let ind = cp_sim::stats::ZONES
        .iter()
        .position(|&z| z == Industrial)
        .unwrap();
    let z = view.city.zones[ind];
    assert!(z.valid > 50 && z.unfit == z.valid, "{z:?}");
    assert!(view.demand.i > 0.0, "{:?}", view.demand);
}

/// The same, to the 10 000 residents of the milestone. Minutes in a debug build, so it only
/// runs on request: `cargo test -p cp_sim --test demand_e2e -- --ignored`.
#[test]
#[ignore = "about 2 minutes in a debug build; the 1000-resident version above is in the suite"]
fn a_zoned_city_grows_to_ten_thousand_residents_unaided() {
    let mut sim = lattice(26, &MIX);
    grow_to(&mut sim, 10_000, 40);
}

#[test]
fn demand_is_part_of_the_state_and_runs_repeat() {
    let run = || {
        let mut sim = lattice(6, &MIX);
        for _ in 0..(2 * TICKS_PER_DAY) {
            sim.tick_once();
        }
        (sim.checksum(), *sim.world().resource::<Demand>())
    };
    let (a, da) = run();
    let (b, db) = run();
    assert_eq!((a, da), (b, db));
    assert_ne!(da, Demand::default(), "the demand moved off its start");
}

#[test]
fn the_view_carries_the_published_demand() {
    let mut sim = lattice(6, &MIX);
    for _ in 0..TICKS_PER_DAY {
        sim.tick_once();
    }
    assert_eq!(sim.view().demand, *sim.world().resource::<Demand>());
    assert_ne!(sim.view().demand, Demand::default());
}

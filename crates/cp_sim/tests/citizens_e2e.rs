//! T-031 acceptance: households move in until the apartments are full, jobs go to the
//! unemployed (education first, then distance), long unemployment and demolition make
//! households leave, and all of it is deterministic.

use std::path::Path;
use std::sync::Arc;

use bevy_ecs::change_detection::Mut;
use bevy_math::{Vec2, Vec3};
use cp_building::{BuildingGrid, BuildingState, demolish_building};
use cp_citizen::{
    ARRIVAL_SPEED_MPS, BASE_DELAY, CitizenGrid, FALLBACK_DELAY, JOBLESS_LEAVE_DAYS, Location,
};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_terrain::Terrain;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// 2x2 block lattice of streets; block `n` is painted `kinds[n % kinds.len()]`.
fn city(content: &Arc<ContentDb>, seed: u64, kinds: &[ZoneType]) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(seed);
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(1.0);
    sim.set_content(Arc::clone(content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(3000.0 + x, 0.0, 3000.0 + z);
        for line in 0..3 {
            let o = line as f32 * 90.0;
            for piece in 0..2 {
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

fn run_days(sim: &mut SimRunner, days: f64) {
    for _ in 0..(days * TICKS_PER_DAY as f64) as u64 {
        sim.tick_once();
    }
}

fn is_res(z: ZoneType) -> bool {
    matches!(z, ZoneType::ResLow | ZoneType::ResHigh)
}

/// Sum of apartments in finished residential buildings.
fn apartments(sim: &SimRunner, db: &ContentDb) -> usize {
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .map(|(_, b)| db.building(b.def))
        .filter(|d| is_res(d.zone))
        .map(|d| usize::from(d.households))
        .sum()
}

#[test]
fn residential_occupancy_grows_to_capacity_and_never_beyond() {
    let content = content();
    let mut sim = city(&content, 5, &[ZoneType::ResHigh, ZoneType::ResLow]);
    let mut last = 0;
    for _ in 0..10 {
        run_days(&mut sim, 0.5);
        let b = sim.world().resource::<BuildingGrid>();
        let c = sim.world().resource::<CitizenGrid>();
        for (id, bd) in b.iter() {
            let def = content.building(bd.def);
            if is_res(def.zone) {
                assert!(
                    c.households_in(id) <= def.households,
                    "{id:?} over capacity"
                );
            }
        }
        last = c.household_count();
    }
    assert!(last > 10, "{last} households");
    // Let the last buildings finish and fill; no jobs exist, so stop before anyone leaves.
    run_days(&mut sim, 2.0);
    let cap = apartments(&sim, &content);
    let c = sim.world().resource::<CitizenGrid>();
    let b = sim.world().resource::<BuildingGrid>();
    let in_active: usize = b
        .iter()
        .filter(|(_, bd)| bd.state == BuildingState::Active)
        .filter(|(_, bd)| is_res(content.building(bd.def).zone))
        .map(|(id, _)| usize::from(c.households_in(id)))
        .sum();
    assert_eq!(
        in_active, cap,
        "every apartment of a finished building is taken"
    );
    // Household sizes are 1..=5 and the first member is an adult.
    for (_, h) in c.households() {
        let n = h.member_ids().count();
        assert!((1..=5).contains(&n));
        assert!(
            c.citizen(h.member_ids().next().unwrap())
                .unwrap()
                .is_adult()
        );
    }
}

/// Industry + residential: more adults than jobs.
fn working_town(content: &Arc<ContentDb>, seed: u64) -> SimRunner {
    city(content, seed, &[ZoneType::ResHigh, ZoneType::Industrial])
}

#[test]
fn unemployment_is_adults_minus_jobs() {
    let content = content();
    let mut sim = working_town(&content, 5);
    run_days(&mut sim, 7.0);
    let jobs_total: usize = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .filter(|(_, b)| b.state == BuildingState::Active)
        .map(|(_, b)| {
            content
                .building(b.def)
                .jobs
                .iter()
                .map(|&j| usize::from(j))
                .sum::<usize>()
        })
        .sum();
    let c = sim.world().resource::<CitizenGrid>();
    let adults = c.adults();
    assert!(
        adults > jobs_total && jobs_total > 20,
        "adults {adults}, jobs {jobs_total}"
    );
    let unemployed = c.unemployed();
    let expected = adults - jobs_total;
    let tolerance = (adults / 100).max(1);
    assert!(
        unemployed.abs_diff(expected) <= tolerance,
        "unemployed {unemployed}, adults - jobs = {expected} (adults {adults}, jobs {jobs_total})"
    );
    // Nobody works somewhere that is full, and workers fill their own education or below.
    let b = sim.world().resource::<BuildingGrid>();
    for (id, bd) in b.iter() {
        let jobs = content.building(bd.def).jobs;
        for (l, &slots) in jobs.iter().enumerate() {
            assert!(c.jobs_filled(id, l) <= slots);
        }
    }
    for (_, cz) in c.citizens() {
        if cz.work.is_some() {
            assert!(cz.work_level <= cz.education);
            assert!(cz.is_adult());
        }
    }
}

#[test]
fn jobs_go_to_matching_education_first_then_to_the_nearest() {
    let content = content();
    let mut sim = working_town(&content, 5);
    run_days(&mut sim, 7.0);
    let c = sim.world().resource::<CitizenGrid>();
    // Some workers got a slot of exactly their education while lower slots still existed.
    let exact = c
        .citizens()
        .filter(|(_, z)| z.work.is_some() && z.work_level == z.education && z.education > 0)
        .count();
    assert!(exact > 0, "educated workers took jobs of their level");
}

#[test]
fn demolishing_a_home_evicts_the_household_and_a_workplace_frees_jobs() {
    let content = content();
    let mut sim = working_town(&content, 5);
    run_days(&mut sim, 6.0);

    // A populated home.
    let (home, hid) = {
        let c = sim.world().resource::<CitizenGrid>();
        let (hid, h) = c
            .households()
            .find(|(_, h)| h.member_ids().count() >= 2)
            .unwrap();
        (h.home, hid)
    };
    let members: Vec<_> = sim
        .world()
        .resource::<CitizenGrid>()
        .household(hid)
        .unwrap()
        .member_ids()
        .collect();
    let before = sim.world().resource::<CitizenGrid>().household_count();
    sim.world_mut()
        .resource_scope(|w, mut b: Mut<BuildingGrid>| {
            let mut z = w.resource_mut::<ZoneGrid>();
            assert!(demolish_building(&mut b, &mut z, home));
        });
    run_days(&mut sim, 0.1); // > 256 ticks: every bucket has been visited
    let c = sim.world().resource::<CitizenGrid>();
    assert!(c.household(hid).is_none(), "household evicted");
    assert!(
        members.iter().all(|m| c.citizen(*m).is_none()),
        "its members are gone"
    );
    assert!(
        c.household_count() <= before,
        "evicted without counting new move-ins as negative"
    );
    assert!(c.households().all(|(_, h)| h.home != home));

    // A workplace with employees.
    let work = {
        let c = sim.world().resource::<CitizenGrid>();
        c.citizens().find_map(|(_, z)| z.work).unwrap()
    };
    let employees: Vec<_> = sim
        .world()
        .resource::<CitizenGrid>()
        .citizens()
        .filter(|(_, z)| z.work == Some(work))
        .map(|(id, _)| id)
        .collect();
    assert!(!employees.is_empty());
    sim.world_mut()
        .resource_scope(|w, mut b: Mut<BuildingGrid>| {
            let mut z = w.resource_mut::<ZoneGrid>();
            demolish_building(&mut b, &mut z, work);
        });
    run_days(&mut sim, 0.1);
    let c = sim.world().resource::<CitizenGrid>();
    for e in employees {
        if let Some(z) = c.citizen(e) {
            assert_ne!(z.work, Some(work), "lost the job");
        }
    }
}

#[test]
fn long_unemployment_makes_a_household_leave() {
    let content = content();
    // Residential only: nobody ever finds work.
    let mut sim = city(&content, 5, &[ZoneType::ResHigh]);
    run_days(&mut sim, 3.0);
    let first = sim.world().resource::<CitizenGrid>().household_count();
    assert!(first > 0);
    // No new arrivals; everyone is jobless and leaves after the grace period.
    *sim.world_mut().resource_mut::<cp_building::Demand>() = cp_building::Demand::fixed(0.0);
    run_days(&mut sim, JOBLESS_LEAVE_DAYS as f64 - 6.0);
    assert!(
        sim.world().resource::<CitizenGrid>().household_count() > 0,
        "still within the grace period"
    );
    run_days(&mut sim, 8.0);
    assert_eq!(sim.world().resource::<CitizenGrid>().household_count(), 0);
    assert_eq!(sim.world().resource::<CitizenGrid>().citizen_count(), 0);
}

#[test]
fn arrival_delay_is_fixed_without_a_map_and_distance_based_with_one() {
    let content = content();
    // Without a map.
    let mut sim = city(&content, 5, &[ZoneType::ResHigh]);
    let created = first_household_tick(&mut sim);
    let c = sim.world().resource::<CitizenGrid>();
    let (_, h) = c.households().next().unwrap();
    assert_eq!(h.arrive_at.0 - created, FALLBACK_DELAY);
    assert!(
        h.member_ids()
            .all(|m| c.citizen(m).unwrap().location == Location::Moving)
    );

    // With a map: base + distance from the nearest external connection at 0.75 m/tick.
    let terrain = Arc::new(
        Terrain::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/maps/flatlands"))
            .unwrap(),
    );
    let mut sim = city(&content, 5, &[ZoneType::ResHigh]);
    sim.set_terrain(Arc::clone(&terrain));
    let created = first_household_tick(&mut sim);
    let c = sim.world().resource::<CitizenGrid>();
    let b = sim.world().resource::<BuildingGrid>();
    let (_, h) = c.households().next().unwrap();
    let pos = b.get(h.home).unwrap().pos;
    let nearest = terrain
        .meta()
        .external_connections
        .iter()
        .map(|&(x, z)| Vec2::new(x, z).distance(pos))
        .fold(f32::MAX, f32::min);
    let expected = BASE_DELAY
        + (nearest / ARRIVAL_SPEED_MPS / cp_core::time::MOVE_SECONDS_PER_TICK).ceil() as u64;
    assert_eq!(h.arrive_at.0 - created, expected);
    assert!(expected > BASE_DELAY);

    // After arrival the members are home and may take jobs.
    run_days(&mut sim, 0.5 + expected as f64 / TICKS_PER_DAY as f64);
    let c = sim.world().resource::<CitizenGrid>();
    assert!(c.citizens().any(|(_, z)| z.location == Location::Home));
}

fn first_household_tick(sim: &mut SimRunner) -> u64 {
    for _ in 0..20_000 {
        sim.tick_once();
        if sim.world().resource::<CitizenGrid>().household_count() > 0 {
            return sim.tick().0 - 1;
        }
    }
    panic!("no household moved in");
}

#[test]
fn same_seed_same_population_and_the_arenas_are_checksummed() {
    let content = content();
    let (mut a, mut b, mut c) = (
        working_town(&content, 5),
        working_town(&content, 5),
        working_town(&content, 6),
    );
    let before = a.checksum();
    for s in [&mut a, &mut b, &mut c] {
        run_days(s, 4.0);
    }
    assert_eq!(a.checksum(), b.checksum());
    assert_eq!(
        a.world().resource::<CitizenGrid>(),
        b.world().resource::<CitizenGrid>()
    );
    assert_ne!(a.checksum(), c.checksum());
    assert_ne!(a.checksum(), before);
    let stats = a.view().stats;
    for name in ["citizens", "households"] {
        let fill = stats.arenas.iter().find(|x| x.name == name).unwrap();
        assert!(fill.used > 0, "{name} gauge");
    }
}

/// Demolishes `old` and immediately puts a copy of it into the freed slot (what a new
/// spawn would do within the eviction window). Returns the new id.
fn replace_in_place(sim: &mut SimRunner, old: cp_building::BuildingId) -> cp_building::BuildingId {
    let copy = sim
        .world()
        .resource::<BuildingGrid>()
        .get(old)
        .unwrap()
        .clone();
    let cells: Vec<ZoneCellRef> = copy.lot.cells().collect();
    sim.world_mut()
        .resource_scope(|w, mut b: Mut<BuildingGrid>| {
            let mut z = w.resource_mut::<ZoneGrid>();
            assert!(demolish_building(&mut b, &mut z, old));
            let new = b.insert(copy);
            z.occupy(&cells);
            new
        })
}

#[test]
fn a_building_reusing_a_removed_slot_starts_with_no_residents() {
    let content = content();
    let mut sim = working_town(&content, 5);
    run_days(&mut sim, 6.0);
    let (hid, home) = {
        let c = sim.world().resource::<CitizenGrid>();
        let (hid, h) = c.households().next().unwrap();
        (hid, h.home)
    };
    let apartments = content
        .building(
            sim.world()
                .resource::<BuildingGrid>()
                .get(home)
                .unwrap()
                .def,
        )
        .households;
    let new = replace_in_place(&mut sim, home);
    assert_eq!(new.index, home.index, "same slot");
    assert_ne!(new.generation, home.generation);
    {
        let c = sim.world().resource::<CitizenGrid>();
        // Not evicted yet, but the new building does not inherit the old residents.
        assert!(c.household(hid).is_some());
        assert_eq!(c.households_in(new), 0);
        assert!(
            c.households_in(home) >= 1,
            "the old id still counts its household"
        );
    }
    for _ in 0..400 {
        sim.tick_once();
        let c = sim.world().resource::<CitizenGrid>();
        assert!(c.households_in(new) <= apartments, "over capacity");
    }
    let c = sim.world().resource::<CitizenGrid>();
    assert!(c.household(hid).is_none(), "the old household was evicted");
    assert!(c.households().all(|(_, h)| h.home != home));
}

#[test]
fn a_workplace_reusing_a_removed_slot_starts_with_no_workers() {
    let content = content();
    let mut sim = working_town(&content, 5);
    run_days(&mut sim, 6.0);
    let work = sim
        .world()
        .resource::<CitizenGrid>()
        .citizens()
        .find_map(|(_, z)| z.work)
        .unwrap();
    let jobs = content
        .building(
            sim.world()
                .resource::<BuildingGrid>()
                .get(work)
                .unwrap()
                .def,
        )
        .jobs;
    let new = replace_in_place(&mut sim, work);
    assert_eq!(new.index, work.index);
    {
        let c = sim.world().resource::<CitizenGrid>();
        for l in 0..4 {
            assert_eq!(c.jobs_filled(new, l), 0, "level {l} starts empty");
        }
    }
    for _ in 0..400 {
        sim.tick_once();
        let c = sim.world().resource::<CitizenGrid>();
        for (l, &slots) in jobs.iter().enumerate() {
            assert!(c.jobs_filled(new, l) <= slots, "level {l} over-filled");
        }
    }
    let c = sim.world().resource::<CitizenGrid>();
    assert!(
        c.citizens().all(|(_, z)| z.work != Some(work)),
        "no one works at the removed building"
    );
}

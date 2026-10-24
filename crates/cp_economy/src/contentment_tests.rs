//! Contentment (T-318a/b, §19.7 items 1, 2, 4, 5): the factors, the complaints, the pass, the
//! move-out and the wellbeing.

use std::sync::Arc;

use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule};
use bevy_ecs::world::World;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Complaint,
    Footprint, HomeContentment, Lot,
};
use cp_citizen::{CitizenGrid, TripBook};
use cp_content::{ContentDb, ContentmentKnobs, ServiceKind, ServiceUnlocks, SimContent};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Id, SimRng, Tick};
use cp_district::{DistrictMap, Districts};
use cp_net::{Elevation, NetGraph, SegmentId};
use cp_service::{ServicePlugin, ServiceProblems, update_coverage};

use crate::contentment::{
    CityContentment, F_COMMUTE, F_ENVIRONMENT, F_LEISURE, F_PROBLEMS, F_SERVICES, F_TAX, F_WORK,
    HomeFacts, complaints, contentment_daily, home_problems, score, step, target,
    update_contentment,
};
use crate::{CITY_PERIOD, Decay, EconomyState, Emissions};

fn knobs() -> ContentmentKnobs {
    ContentmentKnobs::default()
}

/// A home with everything a resident could want.
fn good() -> HomeFacts {
    HomeFacts {
        services: [Some(1.0); 4],
        ground: 0.0,
        noise: 0.0,
        problems: [false; 8],
        jobless: Some(0.0),
        commute_min: Some(10.0),
        green: 0.6,
        leisure_near: true,
        tax_pct: 9,
        trust: 60,
    }
}

fn index_of(f: &HomeFacts) -> (u8, [Complaint; 3]) {
    let k = knobs();
    let s = score(f, &k, 0.3);
    (target(&s.scores, &k.weights), complaints(&s, &k))
}

#[test]
fn a_well_served_clean_green_home_is_content_and_has_no_complaints() {
    let (index, named) = index_of(&good());
    assert!(index >= 80, "{index}");
    assert_eq!(named, [Complaint::None; 3]);
}

#[test]
fn a_town_home_without_services_next_to_industry_is_unhappy_about_both() {
    // Town: Health, Police and Fire are open, none in reach (so garbage piles up and crime
    // grows), no park; a factory 3 cells away (ground 31, noise 37.5 by §11.1). Work, commute,
    // tax and trust are fine: the home is unhappy (below 60, §19.5), not miserable.
    let mut service = ServiceProblems::default();
    service.set(ServiceProblems::GARBAGE, true);
    service.set(ServiceProblems::CRIME, true);
    let f = HomeFacts {
        services: [Some(0.0), Some(0.0), Some(0.0), None],
        ground: 31.0,
        noise: 37.5,
        problems: home_problems(BuildingFlags(0), service),
        green: 0.0,
        leisure_near: false,
        jobless: None,
        commute_min: None,
        ..good()
    };
    let (index, named) = index_of(&f);
    assert!(index < 60, "{index}");
    assert!(index < index_of(&good()).0 - 30, "{index}");
    assert!(named.contains(&Complaint::Pollution), "{named:?}");
    assert!(
        named.iter().any(|c| matches!(
            c,
            Complaint::NoHealth | Complaint::NoPolice | Complaint::NoFire
        )),
        "{named:?}"
    );
}

#[test]
fn services_count_only_once_the_milestone_opened_them() {
    let k = knobs();
    // Hamlet: nothing expected, the factor is full and names nothing.
    let hamlet = HomeFacts {
        services: [None; 4],
        ..good()
    };
    let s = score(&hamlet, &k, 0.3);
    assert_eq!(s.scores[F_SERVICES], 100);
    assert_eq!(s.codes[F_SERVICES], Complaint::None);
    // Village: health, police and fire expected, school not yet; police missing.
    let village = HomeFacts {
        services: [Some(1.0), Some(0.0), Some(1.0), None],
        ..good()
    };
    let s = score(&village, &k, 0.3);
    assert_eq!(s.scores[F_SERVICES], 67);
    assert_eq!(s.codes[F_SERVICES], Complaint::NoPolice);
    // Coverage up to `res_service_cover` counts in proportion.
    let half = HomeFacts {
        services: [Some(0.15), None, None, None],
        ..good()
    };
    assert_eq!(score(&half, &k, 0.3).scores[F_SERVICES], 50);
}

#[test]
fn environment_names_the_larger_of_pollution_and_noise() {
    let k = knobs();
    let loud = HomeFacts {
        ground: 10.0,
        noise: 50.0,
        ..good()
    };
    let s = score(&loud, &k, 0.3);
    assert_eq!(s.scores[F_ENVIRONMENT], 60);
    assert_eq!(s.codes[F_ENVIRONMENT], Complaint::Noise);
    let dirty = HomeFacts {
        ground: 80.0,
        noise: 80.0,
        ..good()
    };
    let s = score(&dirty, &k, 0.3);
    assert_eq!(s.scores[F_ENVIRONMENT], 0);
    assert_eq!(s.codes[F_ENVIRONMENT], Complaint::Pollution);
}

#[test]
fn home_problems_add_their_points_and_the_heaviest_names_them() {
    let k = knobs();
    let flags = BuildingFlags(BuildingFlags::NO_POWER);
    let mut service = ServiceProblems::default();
    service.set(ServiceProblems::GARBAGE, true);
    service.set(ServiceProblems::SICK, true);
    let f = HomeFacts {
        problems: home_problems(flags, service),
        ..good()
    };
    let s = score(&f, &k, 0.3);
    assert_eq!(s.scores[F_PROBLEMS], 100 - 40 - 25 - 10);
    assert_eq!(s.codes[F_PROBLEMS], Complaint::NoPower);
    let cut = HomeFacts {
        problems: home_problems(
            BuildingFlags(BuildingFlags::NO_ROAD | BuildingFlags::NO_WATER),
            ServiceProblems::default(),
        ),
        ..good()
    };
    let s = score(&cut, &k, 0.3);
    assert_eq!(s.scores[F_PROBLEMS], 10);
    assert_eq!(s.codes[F_PROBLEMS], Complaint::NoRoad);
}

#[test]
fn leisure_and_tax_scores() {
    let k = knobs();
    let bare = HomeFacts {
        green: 0.0,
        leisure_near: false,
        ..good()
    };
    assert_eq!(score(&bare, &k, 0.3).scores[F_LEISURE], 0);
    let near = HomeFacts {
        green: 0.2,
        leisure_near: true,
        ..good()
    };
    assert_eq!(score(&near, &k, 0.3).scores[F_LEISURE], 70);
    for (rate, want) in [(1, 100), (9, 100), (12, 70), (19, 0), (29, 0)] {
        let f = HomeFacts {
            tax_pct: rate,
            ..good()
        };
        assert_eq!(score(&f, &k, 0.3).scores[F_TAX], want, "rate {rate}");
    }
}

#[test]
fn complaints_are_the_worst_weighted_factors_below_the_line() {
    let k = knobs();
    let f = HomeFacts {
        services: [Some(0.0); 4],
        ground: 60.0,
        tax_pct: 29,
        green: 0.0,
        leisure_near: false,
        ..good()
    };
    let s = score(&f, &k, 0.3);
    // Services 25 x 100, environment 15 x 60, leisure 10 x 100, tax 5 x 100.
    assert_eq!(
        complaints(&s, &k),
        [Complaint::NoHealth, Complaint::NoPark, Complaint::Pollution]
    );
    // A factor without weight never complains.
    let mut k0 = knobs();
    k0.weights[F_SERVICES] = 0;
    assert!(!complaints(&s, &k0).contains(&Complaint::NoHealth));
}

#[test]
fn the_index_steps_an_eighth_of_the_gap_and_at_least_one_point() {
    assert_eq!(step(50, 90, 8), 55);
    assert_eq!(step(90, 50, 8), 85);
    assert_eq!(step(50, 53, 8), 51);
    assert_eq!(step(53, 50, 8), 52);
    assert_eq!(step(70, 70, 8), 70);
    assert_eq!(step(0, 100, 1), 100);
    assert_eq!(target(&[100; 8], &[0; 8]), 100);
    assert_eq!(
        target(&[100, 0, 0, 0, 0, 0, 0, 0], &[1, 1, 0, 0, 0, 0, 0, 0]),
        50
    );
}

// --- The pass in a world ---

const ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

fn content() -> Arc<ContentDb> {
    let b = |id: &str, zone: &str, svc: &str| {
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: {zone}, level: 1, lot: (1, 1),
            styles: ["p"], service: {svc}))"#
        )
    };
    Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            ("b/house.ron".into(), b("house", "ResLow", "None")),
            ("b/shop.ron".into(), b("shop", "ComLow", "None")),
            (
                "b/clinic.ron".into(),
                b("clinic", "Industrial", "Some(\"clinic\")"),
            ),
            (
                "s/clinic.ron".into(),
                r#"Service((id: "clinic", name: "clinic", kind: Health, radius_m: 150.0,
                    capacity: 100, site: Land, vehicles: []))"#
                    .into(),
            ),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    )
}

struct Town {
    world: World,
    schedule: Schedule,
    segs: Vec<SegmentId>,
}

/// Eight 100 m streets in a row; only Health is open.
fn town() -> Town {
    let db = content();
    let mut world = World::new();
    world.insert_resource(Tick(0));
    world.insert_resource(SimContent(Arc::clone(&db)));
    world.init_resource::<NetGraph>();
    world.init_resource::<BuildingGrid>();
    world.init_resource::<CitizenGrid>();
    world.init_resource::<Emissions>();
    world.init_resource::<EconomyState>();
    world.init_resource::<DistrictMap>();
    world.init_resource::<Districts>();
    world.init_resource::<HomeContentment>();
    world.init_resource::<CityContentment>();
    world.init_resource::<TripBook>();
    world.init_resource::<Decay>();
    world.insert_resource(SimRng::from_seed([7; 32]));
    let mut unlocks = ServiceUnlocks { open_mask: 0 };
    unlocks.set(ServiceKind::Health, true);
    world.insert_resource(unlocks);
    ServicePlugin::init(&mut world);
    let road = db.road_id("street").unwrap();
    let def = db.road(road).clone();
    let mut segs = Vec::new();
    for k in 0..8 {
        let (a, b) = (k as f32 * 100.0, (k + 1) as f32 * 100.0);
        let curve = CubicBezier3::line(Vec3::new(a, 0.0, 0.0), Vec3::new(b, 0.0, 0.0));
        segs.push(
            world
                .resource_mut::<NetGraph>()
                .add_segment(&def, road, curve, Elevation::Ground)
                .unwrap(),
        );
    }
    let mut schedule = Schedule::default();
    schedule.add_systems((update_coverage, update_contentment, contentment_daily).chain());
    Town {
        world,
        schedule,
        segs,
    }
}

impl Town {
    fn put(&mut self, def: &str, seg: usize) -> BuildingId {
        let db = self.world.resource::<SimContent>().0.clone();
        self.world.resource_mut::<BuildingGrid>().insert(Building {
            def: db.building_id(def).unwrap(),
            footprint: Footprint::Lot(Lot {
                block: Id::new(0, 0),
                col: 0,
                w: 1,
                d: 1,
            }),
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos: Vec2::new(seg as f32 * 100.0 + 50.0, 5.0),
            rot: 0.0,
            access: Some(AccessPoint {
                segment: self.segs[seg],
                t: 0.5,
            }),
            flags: BuildingFlags(0),
            level_progress: 0.0,
        })
    }

    /// Runs the passes on the city ticks `1..=n` (coverage first: it runs every 64 ticks).
    fn passes(&mut self, n: u64) {
        for p in 1..=n {
            let t = p * CITY_PERIOD;
            self.world.insert_resource(Tick(t));
            self.schedule.run(&mut self.world);
        }
    }

    fn home(&self, id: BuildingId) -> (u8, [Complaint; 3]) {
        let h = self.world.resource::<HomeContentment>();
        let s = h.get(id).expect("a record");
        (s.index, s.complaints)
    }
}

#[test]
fn the_pass_scores_every_active_home_and_only_homes() {
    let mut t = town();
    t.put("clinic", 0);
    let near = t.put("house", 0);
    let far = t.put("house", 7);
    let shop = t.put("shop", 3);
    t.passes(1);
    let h = t.world.resource::<HomeContentment>();
    assert_eq!(h.len(), 2);
    assert!(h.get(shop).is_none());
    let (near_i, near_c) = t.home(near);
    let (far_i, far_c) = t.home(far);
    assert!(!near_c.contains(&Complaint::NoHealth), "{near_c:?}");
    assert!(far_c.contains(&Complaint::NoHealth), "{far_c:?}");
    // Police, Fire and Education are closed: nobody misses them.
    for c in [Complaint::NoPolice, Complaint::NoFire, Complaint::NoSchool] {
        assert!(!near_c.contains(&c) && !far_c.contains(&c));
    }
    assert!(near_i > far_i, "{near_i} vs {far_i}");
    let city = t.world.resource::<CityContentment>();
    assert_eq!(city.homes, 2);
    assert_eq!(city.districts.len(), 1);
    assert_eq!(city.districts[0].district, None);
    assert_eq!(city.districts[0].homes, 2);
    assert!(
        city.complaints.contains(&(Complaint::NoHealth, 1)),
        "{:?}",
        city.complaints
    );
    // Unweighted while nobody lives there: the plain mean.
    assert_eq!(
        city.city,
        (u32::from(near_i) + u32::from(far_i)).div_ceil(2) as u8
    );
}

#[test]
fn a_home_starts_at_its_target_then_moves_by_steps() {
    let mut t = town();
    let clinic = t.put("clinic", 0);
    let home = t.put("house", 0);
    t.passes(1);
    let (start, _) = t.home(home);
    // The clinic goes: the target drops, the index follows an eighth of the gap a pass.
    t.world.resource_mut::<BuildingGrid>().remove(clinic);
    t.world.insert_resource(Tick(2 * CITY_PERIOD));
    t.schedule.run(&mut t.world);
    let (next, named) = t.home(home);
    assert!(next < start, "{next} vs {start}");
    assert!(start - next < 10, "a step, not a jump: {start} -> {next}");
    assert!(named.contains(&Complaint::NoHealth), "{named:?}");
}

#[test]
fn a_demolished_home_loses_its_record_and_the_pass_is_deterministic() {
    let run = || {
        let mut t = town();
        t.put("clinic", 2);
        let homes: Vec<BuildingId> = (0..8).map(|s| t.put("house", s)).collect();
        t.passes(3);
        t.world.resource_mut::<BuildingGrid>().remove(homes[4]);
        t.world.insert_resource(Tick(4 * CITY_PERIOD));
        t.schedule.run(&mut t.world);
        assert!(
            t.world
                .resource::<HomeContentment>()
                .get(homes[4])
                .is_none()
        );
        assert_eq!(t.world.resource::<HomeContentment>().len(), 7);
        (
            t.world.resource::<HomeContentment>().clone(),
            t.world.resource::<CityContentment>().clone(),
        )
    };
    assert_eq!(run(), run());
}

#[test]
fn the_pass_runs_only_on_city_ticks() {
    let mut t = town();
    t.put("house", 1);
    for tick in [0, 1, 64, CITY_PERIOD - 1] {
        t.world.insert_resource(Tick(tick));
        t.schedule.run(&mut t.world);
    }
    assert!(t.world.resource::<HomeContentment>().is_empty());
    t.passes(1);
    assert_eq!(t.world.resource::<HomeContentment>().len(), 1);
}

/// An adult household (`n` members) moving into `home` at tick 0.
fn household(t: &mut Town, home: BuildingId, n: usize) -> cp_citizen::HouseholdId {
    let mut grid = t.world.resource_mut::<CitizenGrid>();
    grid.ensure_derived();
    grid.add_household(home, 0, Tick(0), &vec![(30 * 365, 1, false); n])
}

#[test]
fn work_and_commute_come_from_the_households_and_the_trips_to_work() {
    let mut t = town();
    t.put("clinic", 0);
    let home = t.put("house", 0);
    let a = household(&mut t, home, 1);
    household(&mut t, home, 1);
    // One of the two households jobless from the start; it counts once it has minded long
    // enough (3 days).
    t.world
        .resource_mut::<CitizenGrid>()
        .set_jobless_since(a, Some(Tick(0)));
    t.passes(1);
    let city = t.world.resource::<CityContentment>();
    assert_eq!(city.factors[F_WORK], 100);
    assert_eq!(city.factors[F_COMMUTE], 100, "no trip to work seen yet");
    // Trips to work of 70 minutes and more: the average is past the bad line.
    t.world
        .resource_mut::<TripBook>()
        .commutes
        .extend([(home, 70), (home, 80)]);
    let days3 = 3 * TICKS_PER_DAY / CITY_PERIOD;
    t.passes(days3 + 1);
    assert!(
        t.world.resource::<TripBook>().commutes.is_empty(),
        "folded in"
    );
    let slot = *t.world.resource::<HomeContentment>().get(home).unwrap();
    assert_eq!(slot.commute_min, 71);
    let city = t.world.resource::<CityContentment>();
    assert_eq!(city.factors[F_WORK], 50);
    assert_eq!(city.factors[F_COMMUTE], 0);
    assert!(
        slot.complaints.contains(&Complaint::LongCommute),
        "{:?}",
        slot.complaints
    );
    // Short trips bring it back down.
    t.world
        .resource_mut::<TripBook>()
        .commutes
        .extend(std::iter::repeat_n((home, 6u16), 40));
    t.passes(days3 + 2);
    let slot = *t.world.resource::<HomeContentment>().get(home).unwrap();
    assert!(slot.commute_min <= 15, "{}", slot.commute_min);
    assert!(!slot.complaints.contains(&Complaint::LongCommute));
}

#[test]
fn a_home_below_the_line_gets_a_timer_that_goes_when_it_recovers() {
    let mut t = town();
    t.put("clinic", 0);
    let home = t.put("house", 0);
    t.passes(1);
    // Forced miserable: the pass moves it an eighth of the way up, still below 20.
    t.world
        .resource_mut::<HomeContentment>()
        .set(home, 2, [Complaint::None; 3]);
    t.world.insert_resource(Tick(2 * CITY_PERIOD));
    t.schedule.run(&mut t.world);
    let h = t.world.resource::<HomeContentment>();
    assert!(h.index_of(home) < 20);
    assert_eq!(h.unhappy_since.get(&home), Some(&Tick(2 * CITY_PERIOD)));
    // The timer keeps its start while the home stays below, and goes once it is above.
    let mut pass = 3;
    while t.world.resource::<HomeContentment>().index_of(home) < 20 {
        t.world.insert_resource(Tick(pass * CITY_PERIOD));
        t.schedule.run(&mut t.world);
        let h = t.world.resource::<HomeContentment>();
        if h.index_of(home) < 20 {
            assert_eq!(h.unhappy_since.get(&home), Some(&Tick(2 * CITY_PERIOD)));
        }
        pass += 1;
    }
    assert!(
        t.world
            .resource::<HomeContentment>()
            .unhappy_since
            .is_empty()
    );
}

/// Runs only the daily system on days `1..=days`, the home held below the line since tick 0.
fn unhappy_days(decay: bool, days: u64) -> (u16, u16) {
    let mut t = town();
    let home = t.put("house", 0);
    let other = t.put("house", 1);
    for _ in 0..6 {
        household(&mut t, home, 2);
        household(&mut t, other, 2);
    }
    {
        let mut h = t.world.resource_mut::<HomeContentment>();
        h.set(home, 10, [Complaint::None; 3]);
        h.set(other, 70, [Complaint::None; 3]);
        h.unhappy_since.insert(home, Tick(0));
    }
    if !decay {
        t.world.insert_resource(Decay::disabled());
    }
    let mut daily = Schedule::default();
    daily.add_systems(contentment_daily);
    for d in 1..=days {
        t.world.insert_resource(Tick(d * TICKS_PER_DAY));
        daily.run(&mut t.world);
    }
    let grid = t.world.resource::<CitizenGrid>();
    (grid.households_in(home), grid.households_in(other))
}

#[test]
fn an_unhappy_home_loses_households_after_its_days_and_never_in_the_test_mode() {
    // Not before `leave_days` (10).
    assert_eq!(unhappy_days(true, 9), (6, 6));
    let (left, other) = unhappy_days(true, 30);
    // 21 draws (days 10..=30) at a 0.3 chance, one household each.
    assert!(left < 6, "{left}");
    assert_eq!(other, 6, "the content home keeps its households");
    assert_eq!(unhappy_days(false, 30), (6, 6));
    assert_eq!(unhappy_days(true, 30), (left, other), "deterministic");
}

#[test]
fn wellbeing_is_the_home_index_less_the_personal_troubles() {
    let mut t = town();
    let home = t.put("house", 0);
    let well = household(&mut t, home, 1);
    let ill = household(&mut t, home, 1);
    let idle = household(&mut t, home, 1);
    t.world
        .resource_mut::<HomeContentment>()
        .set(home, 64, [Complaint::None; 3]);
    let member = |t: &Town, h| {
        t.world
            .resource::<CitizenGrid>()
            .household(h)
            .unwrap()
            .member_ids()
            .next()
            .unwrap()
    };
    let (w, i, j) = (member(&t, well), member(&t, ill), member(&t, idle));
    {
        let mut grid = t.world.resource_mut::<CitizenGrid>();
        grid.set_sick(i, Some(Tick(0)));
        grid.set_jobless_since(idle, Some(Tick(0)));
    }
    let mut daily = Schedule::default();
    daily.add_systems(contentment_daily);
    t.world.insert_resource(Tick(4 * TICKS_PER_DAY));
    daily.run(&mut t.world);
    let grid = t.world.resource::<CitizenGrid>();
    let wb = |c| grid.citizen(c).unwrap().wellbeing;
    assert_eq!((wb(w), wb(i), wb(j)), (64, 49, 44));
}

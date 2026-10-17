//! T-200: unique buildings and monuments. A unique def waits for its conditions and stays open
//! once they held, only one of each may stand, it lifts the land value around it and brings
//! tourists; the opened set round-trips through a save and replays.

use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_ecs::system::RunSystemOnce;
use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingCommand, BuildingError, BuildingFlags, BuildingGrid,
    BuildingState, Footprint, Lot,
};
use cp_citizen::CitizenGrid;
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{CommandError, CommandOutcome, Id, Tick};
use cp_economy::LandGrids;
use cp_net::{Elevation, NetGraph, NodeKind};
use cp_sim::progress::update_progress;
use cp_sim::{PROGRESS_PERIOD, Progress, SimCommand, SimRunner};
use cp_terrain::TileOwnership;
use cp_transit::Tourists;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn world(content: &Arc<ContentDb>) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(1);
    sim.set_content(Arc::clone(content));
    sim.world_mut().insert_resource(TileOwnership::only((1, 1)));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3200.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim.tick_once();
    sim.world_mut().insert_resource(Progress::START);
    sim.world_mut().resource_mut::<cp_core::Treasury>().balance =
        cp_core::Cost::from_units(50_000_000.0);
    sim
}

fn spot(sim: &SimRunner, col: u16) -> Vec2 {
    let zones = sim.world().resource::<ZoneGrid>();
    let (id, _) = zones.blocks().next().unwrap();
    zones
        .cell_footprint(ZoneCellRef {
            block: id,
            cell: col * 4 + 1,
        })
        .unwrap()
        .c
}

fn place(sim: &mut SimRunner, name: &str, col: u16) -> Result<(), CommandError> {
    let def = sim
        .world()
        .resource::<cp_content::SimContent>()
        .0
        .building_id(name)
        .unwrap();
    let pos = spot(sim, col);
    sim.push_command(
        SimCommand::Building(BuildingCommand::Place { def, pos, rot: 0.0 }),
        sim.tick(),
    );
    sim.apply_due_commands();
    let out: Vec<CommandOutcome> = sim
        .world_mut()
        .resource_mut::<Messages<CommandOutcome>>()
        .drain()
        .collect();
    out.last().unwrap().result.clone().map(|_| ())
}

/// `n` residents (six to a household) living in the first building.
fn residents(sim: &mut SimRunner, n: u32) {
    let home = sim
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .next()
        .map(|(id, _)| id)
        .unwrap_or(Id::new(0, 0));
    let now = sim.world().resource::<Tick>().0;
    let members = vec![(30 * cp_citizen::DAYS_PER_YEAR, 1, false); 6];
    let mut g = sim.world_mut().resource_mut::<CitizenGrid>();
    for _ in 0..n / 6 {
        g.add_household(home, 0, Tick(now), &members);
    }
}

/// Runs the unlock pass on a pass tick (the sim would evict homeless residents first).
fn pass(sim: &mut SimRunner, k: u64) {
    *sim.world_mut().resource_mut::<Tick>() = Tick(k * PROGRESS_PERIOD);
    sim.world_mut().run_system_once(update_progress).unwrap();
}

fn is_locked(r: &Result<(), CommandError>) -> bool {
    matches!(r, Err(CommandError::Locked { .. }))
}

#[test]
fn the_plaza_waits_for_1500_residents_and_stays_open_when_the_city_shrinks() {
    let c = content();
    let mut sim = world(&c);
    // A first building to live in (no milestone names the coal plant).
    place(&mut sim, "coal_power_plant", 12).unwrap();
    let count = |sim: &SimRunner| sim.world().resource::<CitizenGrid>().citizen_count();
    while count(&sim) < 1000 {
        residents(&mut sim, 60);
    }
    pass(&mut sim, 1);
    let n = count(&sim);
    assert!((1000..1500).contains(&n), "{n} residents");
    assert!(is_locked(&place(&mut sim, "city_plaza", 2)));
    assert!(
        !sim.world()
            .resource::<Progress>()
            .unlocked
            .contains("city_plaza")
    );

    while count(&sim) < 1500 {
        residents(&mut sim, 60);
    }
    pass(&mut sim, 2);
    assert!(
        sim.world()
            .resource::<Progress>()
            .unlocked
            .contains("city_plaza")
    );
    place(&mut sim, "city_plaza", 2).unwrap();

    // The status for the UI.
    let view = sim.view();
    let st = view.uniques.iter().find(|u| u.id == "city_plaza").unwrap();
    assert!(st.unlocked && st.built && !st.monument);
    assert!(st.conditions.iter().all(|&(_, cur, need)| cur >= need));
    let arch = view.uniques.iter().find(|u| u.id == "golden_arch").unwrap();
    assert!(!arch.unlocked && !arch.built && arch.monument);

    // The residents leave; the plaza stays open.
    sim.world_mut().insert_resource(CitizenGrid::default());
    pass(&mut sim, 3);
    assert!(
        sim.world()
            .resource::<Progress>()
            .unlocked
            .contains("city_plaza")
    );
}

#[test]
fn a_second_copy_is_refused_even_while_the_first_is_building() {
    let c = content();
    let mut sim = world(&c);
    sim.world_mut().insert_resource(Progress::ALL);
    place(&mut sim, "city_plaza", 2).unwrap();
    let err = place(&mut sim, "city_plaza", 8);
    let text = format!("{err:?}");
    assert!(
        text.contains(&format!("{:?}", BuildingError::UniqueBuilt)) || err.is_err(),
        "{text}"
    );
    assert!(err.is_err());
    assert_eq!(sim.world().resource::<BuildingGrid>().len(), 1);
}

#[test]
fn the_golden_arch_opens_after_four_uniques_stand() {
    let c = content();
    let mut sim = world(&c);
    sim.world_mut().insert_resource(Progress::ALL);
    for (i, name) in ["city_plaza", "clock_tower", "grand_library", "opera_house"]
        .iter()
        .enumerate()
    {
        place(&mut sim, name, 1 + 5 * i as u16).unwrap();
    }
    sim.world_mut().insert_resource(Progress::START);
    assert!(is_locked(&place(&mut sim, "golden_arch", 21)));
    pass(&mut sim, 1);
    assert!(
        sim.world()
            .resource::<Progress>()
            .unlocked
            .contains("golden_arch")
    );
}

#[test]
fn a_unique_building_lifts_the_land_value_inside_its_radius_only() {
    let c = content();
    let value = |with: bool| {
        let mut sim = world(&c);
        sim.world_mut().insert_resource(Progress::ALL);
        if with {
            place(&mut sim, "city_plaza", 2).unwrap();
        }
        for _ in 0..300 {
            sim.tick_once();
        }
        let p = spot(&sim, 2);
        let g = sim.world().resource::<LandGrids>();
        (g.value_at(p), g.value_at(p + Vec2::new(0.0, 600.0)))
    };
    let (near_with, far_with) = value(true);
    let (near_without, far_without) = value(false);
    assert!(near_with > near_without, "{near_with} vs {near_without}");
    assert_eq!(far_with, far_without);
}

#[test]
fn a_unique_building_with_tourists_is_a_goal_and_adds_to_the_flow() {
    let db = Arc::new(
        ContentDb::from_sources(vec![
            ("r/street.ron".into(), TOWN_ROAD.into()),
            ("s/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
            (
                "b/shop.ron".into(),
                r#"Building((id: "shop", name: "shop", zone: ComLow, level: 1, lot: (1, 1),
                styles: ["p"], service: None))"#
                    .into(),
            ),
            (
                "b/arch.ron".into(),
                r#"Building((id: "arch", name: "arch", zone: ComLow, level: 1, lot: (1, 1),
                styles: ["p"], service: None,
                unique: Some((monument: true, unlock: [], tourists_per_day: 200.0))))"#
                    .into(),
            ),
            (
                "v/car.ron".into(),
                r#"Vehicle((id: "car", name: "car", kind: Car, length: 4.5, width: 1.8,
                max_speed_kmh: 60.0, accel: 2.0, decel: 4.0, min_gap: 2.0, mesh: "m"))"#
                    .into(),
            ),
        ])
        .unwrap_or_else(|e| panic!("{e}")),
    );
    let visits = |with_arch: bool| {
        let mut sim = SimRunner::new(7);
        sim.set_content(Arc::clone(&db));
        let road = db.road_id("street").unwrap();
        let def = db.road(road).clone();
        sim.world_mut().resource_mut::<NetGraph>().add_node(
            Vec3::ZERO,
            NodeKind::Outside,
            Elevation::Ground,
        );
        let seg = sim
            .world_mut()
            .resource_mut::<NetGraph>()
            .add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0)),
                Elevation::Ground,
            )
            .unwrap();
        let put = |sim: &mut SimRunner, name: &str, at: f32| {
            sim.world_mut()
                .resource_mut::<BuildingGrid>()
                .insert(Building {
                    def: db.building_id(name).unwrap(),
                    footprint: Footprint::Lot(Lot {
                        block: Id::new(0, 0),
                        col: 0,
                        w: 1,
                        d: 1,
                    }),
                    level: 1,
                    state: BuildingState::Active,
                    ready_at: Tick(0),
                    pos: Vec2::new(at, 5.0),
                    rot: 0.0,
                    access: Some(AccessPoint {
                        segment: seg,
                        t: at / 1000.0,
                    }),
                    flags: BuildingFlags(0),
                    level_progress: 0.0,
                })
        };
        for i in 0..5 {
            put(&mut sim, "shop", 150.0 + 80.0 * i as f32);
        }
        let arch = with_arch.then(|| put(&mut sim, "arch", 700.0));
        sim.tick_once();
        for _ in 0..16_000 {
            sim.tick_once();
        }
        let t = sim.world().resource::<Tourists>();
        let total: u32 = t.customers.values().sum();
        let at_arch = arch.map_or(0, |a| t.customers.get(&a).copied().unwrap_or(0));
        (total, at_arch)
    };
    let (plain, _) = visits(false);
    let (with, at_arch) = visits(true);
    assert!(at_arch > 0, "nobody came to the arch");
    assert!(with > plain, "{with} vs {plain}");
}

const TOWN_ROAD: &str = r#"Road((id: "street", name: "street", width: 12.0,
    lanes: [
        (kind: Car, dir: Backward, width: 3.5, offset: 1.75),
        (kind: Car, dir: Forward, width: 3.5, offset: -1.75),
    ],
    median_width: 0.0, speed_kmh: 40, max_slope: 0.12, cost_per_m: 10.0,
    upkeep_per_m_week: 0.5, allows_zoning: true, rank: 1, elevated: None, bridge: None,
    mesh: "m", material: "m"))"#;

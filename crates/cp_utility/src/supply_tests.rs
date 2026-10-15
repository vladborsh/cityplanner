//! Supply tests on a tiny custom content set so capacities are easy to reason about:
//! a house needs 10 of water, sewage and power; a pump gives 25 water, an outflow 100
//! sewage, a plant 25 power.

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingId, BuildingState, Lot,
};
use cp_content::ContentDb;
use cp_core::curve::CubicBezier3;
use cp_core::{Id, Tick};

use super::*;

fn building_src(id: &str, svc: Option<&str>, cons: &str) -> (String, String) {
    let service = svc.map_or("None".to_string(), |s| format!("Some(\"{s}\")"));
    (
        format!("b/{id}.ron"),
        format!(
            r#"Building((id: "{id}", name: "{id}", zone: ResLow, level: 1, lot: (1, 1),
            consumption: {cons}, tax_base: 0.0,
            style: (floors: 1, roof: Flat, palette: "p"), service: {service}))"#
        ),
    )
}

fn service_src(id: &str, kind: &str, cap: u32) -> (String, String) {
    (
        format!("s/{id}.ron"),
        format!(
            r#"Service((id: "{id}", name: "{id}", kind: {kind}, radius_m: 100.0, capacity: {cap}, vehicles: []))"#
        ),
    )
}

fn db() -> ContentDb {
    ContentDb::from_sources(vec![
        building_src("house", None, "{Water: 10.0, Sewage: 10.0, Power: 10.0}"),
        building_src("pump", Some("pump"), "{}"),
        building_src("outflow", Some("outflow"), "{}"),
        building_src("plant", Some("plant"), "{}"),
        building_src("tower", Some("tower"), "{Power: 10.0}"),
        service_src("tower", "Water", 300),
        service_src("pump", "Water", 25),
        service_src("outflow", "Sewage", 100),
        service_src("plant", "Power", 25),
    ])
    .unwrap_or_else(|e| panic!("{e}"))
}

fn add(grid: &mut BuildingGrid, db: &ContentDb, def: &str, x: f32, z: f32) -> BuildingId {
    grid.insert(Building {
        def: db.building_id(def).unwrap(),
        footprint: cp_building::Footprint::Lot(Lot {
            block: Id::new(0, 0),
            col: 0,
            w: 1,
            d: 1,
        }),
        level: 1,
        state: BuildingState::Active,
        ready_at: Tick(0),
        pos: Vec2::new(5000.0 + x, 5000.0 + z),
        rot: 0.0,
        access: Some(AccessPoint {
            segment: Id::new(0, 0),
            t: 0.0,
        }),
        flags: BuildingFlags::default(),
        level_progress: 0.0,
    })
}

/// A straight line of `net` from `x0` to `x1` at `z = 0`.
fn line(g: &mut UtilityGraph, net: UtilityNet, x0: f32, x1: f32) {
    let p = |x: f32| Vec3::new(5000.0 + x, 0.0, 5000.0);
    let a = g.add_node(net, p(x0));
    let b = g.add_node(net, p(x1));
    g.add_link(a, b, CubicBezier3::line(p(x0), p(x1))).unwrap();
}

fn bits(m: &std::collections::BTreeMap<BuildingId, u8>, id: BuildingId) -> u8 {
    m.get(&id).copied().unwrap_or(0)
}

#[test]
fn a_water_deficit_cuts_the_highest_ids_first_and_the_order_is_stable() {
    let db = db();
    let mut b = BuildingGrid::new();
    let mut g = UtilityGraph::new();
    line(&mut g, UtilityNet::Pipe, 0.0, 200.0);
    let pump = add(&mut b, &db, "pump", 0.0, 10.0);
    let houses: Vec<_> = (0..5)
        .map(|i| add(&mut b, &db, "house", 20.0 + 30.0 * i as f32, 15.0))
        .collect();
    let first = compute_supply(&db, &b, &g);
    for _ in 0..5 {
        assert_eq!(compute_supply(&db, &b, &g), first, "stable");
    }
    // 25 water, 10 each: two houses fit; the third and everything after lose water.
    for (i, h) in houses.iter().enumerate() {
        let no_water = bits(&first, *h) & BuildingFlags::NO_WATER != 0;
        assert_eq!(no_water, i >= 2, "house {i}");
    }
    assert_eq!(bits(&first, pump), 0);
}

#[test]
fn a_building_far_from_the_pipe_has_no_water() {
    let db = db();
    let mut b = BuildingGrid::new();
    let mut g = UtilityGraph::new();
    line(&mut g, UtilityNet::Pipe, 0.0, 100.0);
    add(&mut b, &db, "pump", 0.0, 10.0);
    let near = add(&mut b, &db, "house", 50.0, 39.0);
    let far = add(&mut b, &db, "house", 50.0, 45.0);
    let m = compute_supply(&db, &b, &g);
    assert_eq!(bits(&m, near) & BuildingFlags::NO_WATER, 0);
    assert_ne!(bits(&m, far) & BuildingFlags::NO_WATER, 0);
}

#[test]
fn an_outflow_without_a_connection_to_water_does_not_work() {
    let db = db();
    let mut b = BuildingGrid::new();
    let mut g = UtilityGraph::new();
    line(&mut g, UtilityNet::Pipe, 0.0, 100.0);
    add(&mut b, &db, "outflow", 100.0, 10.0);
    let h = add(&mut b, &db, "house", 50.0, 10.0);
    let m = compute_supply(&db, &b, &g);
    assert_ne!(
        bits(&m, h) & BuildingFlags::NO_SEWAGE,
        0,
        "no pump: no sewage"
    );
    // Connect a pump to the same pipe component: the outflow works.
    add(&mut b, &db, "pump", 0.0, 10.0);
    let m = compute_supply(&db, &b, &g);
    assert_eq!(bits(&m, h) & BuildingFlags::NO_SEWAGE, 0);
    // A pump on a separate pipe does not help.
    let mut b2 = BuildingGrid::new();
    let mut g2 = UtilityGraph::new();
    line(&mut g2, UtilityNet::Pipe, 0.0, 100.0);
    let far = |x: f32| Vec3::new(5000.0 + x, 0.0, 5500.0);
    let (p0, p1) = (
        g2.add_node(UtilityNet::Pipe, far(0.0)),
        g2.add_node(UtilityNet::Pipe, far(100.0)),
    );
    g2.add_link(p0, p1, CubicBezier3::line(far(0.0), far(100.0)))
        .unwrap();
    add(&mut b2, &db, "outflow", 100.0, 10.0);
    add(&mut b2, &db, "pump", 0.0, 500.0);
    let h2 = add(&mut b2, &db, "house", 50.0, 10.0);
    let m = compute_supply(&db, &b2, &g2);
    assert_ne!(bits(&m, h2) & BuildingFlags::NO_SEWAGE, 0);
}

#[test]
fn power_deficit_and_the_30_m_hop() {
    let db = db();
    let mut b = BuildingGrid::new();
    let mut g = UtilityGraph::new();
    line(&mut g, UtilityNet::Power, 0.0, 100.0);
    add(&mut b, &db, "plant", 0.0, 10.0);
    // On the line: ids ascending; 25 power feeds two houses.
    let on_line: Vec<_> = (0..3)
        .map(|i| add(&mut b, &db, "house", 30.0 + 20.0 * i as f32, 20.0))
        .collect();
    let m = compute_supply(&db, &b, &g);
    assert_eq!(bits(&m, on_line[0]) & BuildingFlags::NO_POWER, 0);
    assert_eq!(bits(&m, on_line[1]) & BuildingFlags::NO_POWER, 0);
    assert_ne!(bits(&m, on_line[2]) & BuildingFlags::NO_POWER, 0);

    // With a second plant there is room for a house 25 m past the line's reach, passed on by
    // a supplied neighbour; one 60 m away gets nothing.
    let mut b = BuildingGrid::new();
    add(&mut b, &db, "plant", 0.0, 10.0);
    let a = add(&mut b, &db, "house", 50.0, 35.0);
    let hopped = add(&mut b, &db, "house", 50.0, 62.0);
    let lonely = add(&mut b, &db, "house", 50.0, 120.0);
    let m = compute_supply(&db, &b, &g);
    assert_eq!(bits(&m, a) & BuildingFlags::NO_POWER, 0);
    assert_eq!(bits(&m, hopped) & BuildingFlags::NO_POWER, 0, "hop of 27 m");
    assert_ne!(bits(&m, lonely) & BuildingFlags::NO_POWER, 0);
}

#[test]
fn the_system_writes_flags_every_64_ticks_only() {
    use bevy_ecs::prelude::*;
    let db = db();
    let mut world = World::new();
    world.insert_resource(Tick(1));
    world.insert_resource(cp_content::SimContent(std::sync::Arc::new(db)));
    world.insert_resource(UtilityGraph::new());
    let mut grid = BuildingGrid::new();
    let h = {
        let db = &world.resource::<cp_content::SimContent>().0;
        add(&mut grid, db, "house", 0.0, 0.0)
    };
    world.insert_resource(grid);
    let mut sched = Schedule::default();
    sched.add_systems(update_supply);
    sched.run(&mut world);
    assert_eq!(world.resource::<BuildingGrid>().get(h).unwrap().flags.0, 0);
    world.insert_resource(Tick(64));
    sched.run(&mut world);
    let f = world.resource::<BuildingGrid>().get(h).unwrap().flags;
    assert!(f.has(BuildingFlags::NO_WATER) && f.has(BuildingFlags::NO_POWER));
    assert!(f.has(BuildingFlags::NO_SEWAGE));
}

/// The coverage band the player sees is `BUILDING_REACH` wide: that is the supply rule.
#[test]
fn the_coverage_band_half_width_is_the_supply_reach() {
    assert_eq!(plan::BUILDING_REACH, LINE_REACH_M);
    let db = db();
    let mut b = BuildingGrid::new();
    let mut g = UtilityGraph::new();
    line(&mut g, UtilityNet::Pipe, 0.0, 100.0);
    add(&mut b, &db, "pump", 0.0, 10.0);
    let inside = add(&mut b, &db, "house", 50.0, plan::BUILDING_REACH - 1.0);
    let outside = add(&mut b, &db, "house", 50.0, plan::BUILDING_REACH + 1.0);
    let m = compute_supply(&db, &b, &g);
    assert_eq!(
        bits(&m, inside) & BuildingFlags::NO_WATER,
        0,
        "39 m: connected"
    );
    assert_ne!(bits(&m, outside) & BuildingFlags::NO_WATER, 0, "41 m: not");
}

/// T-109: a water tower (a free-standing water source of 300 that itself needs 10 power) with
/// a pipe within 40 m supplies the houses in the pipe's component up to its capacity, works
/// without electricity, and makes the component's outflow work like a pump does.
#[test]
fn a_water_tower_supplies_its_pipe_component_without_power() {
    let db = db();
    let mut b = BuildingGrid::new();
    let mut g = UtilityGraph::new();
    line(&mut g, UtilityNet::Pipe, 0.0, 200.0);
    // 35 m from the pipe, standing free (no lot, no road).
    let tower = add(&mut b, &db, "tower", 100.0, 35.0);
    {
        let t = b.get_mut(tower).unwrap();
        t.footprint = cp_building::Footprint::Free { w: 2, d: 2 };
        t.access = None;
    }
    let houses: Vec<_> = (0..31)
        .map(|i| add(&mut b, &db, "house", 5.0 * i as f32, 10.0))
        .collect();
    let m = compute_supply(&db, &b, &g);
    for (i, h) in houses.iter().enumerate() {
        let dry = bits(&m, *h) & BuildingFlags::NO_WATER != 0;
        assert_eq!(dry, i >= 30, "house {i}: 300 water, 10 each");
    }
    assert_ne!(
        bits(&m, tower) & BuildingFlags::NO_POWER,
        0,
        "no plant: the tower has no power and still pumps"
    );
    assert_eq!(bits(&m, tower) & BuildingFlags::NO_WATER, 0);

    // Out of reach of the pipe (45 m): it gives nothing.
    let mut b2 = BuildingGrid::new();
    let far = add(&mut b2, &db, "tower", 100.0, 45.0);
    let h = add(&mut b2, &db, "house", 100.0, 10.0);
    let m2 = compute_supply(&db, &b2, &g);
    assert_ne!(bits(&m2, h) & BuildingFlags::NO_WATER, 0);
    let _ = far;

    // An outflow in the component works because the tower is a working water source.
    let outflow = add(&mut b, &db, "outflow", 150.0, 10.0);
    let m3 = compute_supply(&db, &b, &g);
    assert_eq!(bits(&m3, houses[0]) & BuildingFlags::NO_SEWAGE, 0);
    let _ = outflow;
}

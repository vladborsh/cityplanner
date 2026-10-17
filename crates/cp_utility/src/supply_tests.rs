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
            styles: ["p"], service: {service}))"#
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
    db_with("")
}

/// The test content with an extra `balance.ron` body (`""` = none: the default `networks`).
fn db_with(balance: &str) -> ContentDb {
    let mut extra = Vec::new();
    if !balance.is_empty() {
        extra.push(("balance.ron".to_string(), format!("Balance(({balance}))")));
    }
    ContentDb::from_sources(
        extra
            .into_iter()
            .chain([
                ("style/p.ron".into(), cp_content::TEST_STYLE_RON.into()),
                building_src("house", None, "{Water: 10.0, Sewage: 10.0, Power: 10.0}"),
                building_src("pump", Some("pump"), "{}"),
                building_src("outflow", Some("outflow"), "{}"),
                building_src("plant", Some("plant"), "{}"),
                building_src("tower", Some("tower"), "{Power: 10.0}"),
                service_src("tower", "Water", 300),
                building_src("treatment", Some("treatment"), "{Power: 40.0}"),
                service_src("treatment", "Sewage", 1500),
                service_src("pump", "Water", 25),
                service_src("outflow", "Sewage", 100),
                service_src("plant", "Power", 25),
                building_src("bigplant", Some("bigplant"), "{}"),
                service_src("bigplant", "Power", 100),
            ])
            .collect(),
    )
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
    let near = add(&mut b, &db, "house", 50.0, 60.0);
    let far = add(&mut b, &db, "house", 50.0, 70.0);
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

    // A house past the line's reach is passed the current by a supplied neighbour 19 m away
    // edge to edge (BUG-036: edges, not centres); one far away gets nothing.
    let mut b = BuildingGrid::new();
    add(&mut b, &db, "plant", 0.0, 10.0);
    let a = add(&mut b, &db, "house", 50.0, 35.0);
    let hopped = add(&mut b, &db, "house", 50.0, 62.0);
    let lonely = add(&mut b, &db, "house", 50.0, 120.0);
    let m = compute_supply(&db, &b, &g);
    assert_eq!(bits(&m, a) & BuildingFlags::NO_POWER, 0);
    assert_eq!(
        bits(&m, hopped) & BuildingFlags::NO_POWER,
        0,
        "edge gap of 19 m"
    );
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

/// The coverage band the player sees is `pipe_reach_m` wide: that is the supply rule.
#[test]
fn the_coverage_band_half_width_is_the_supply_reach() {
    let db = db();
    let reach = db.balance().networks.pipe_reach_m;
    let mut b = BuildingGrid::new();
    let mut g = UtilityGraph::new();
    line(&mut g, UtilityNet::Pipe, 0.0, 100.0);
    add(&mut b, &db, "pump", 0.0, 10.0);
    // A 1x1 lot is 8 m wide: its edge counts, so its centre may be 4 m further out.
    let inside = add(&mut b, &db, "house", 50.0, reach + 3.0);
    let outside = add(&mut b, &db, "house", 50.0, reach + 5.0);
    let m = compute_supply(&db, &b, &g);
    assert_eq!(
        bits(&m, inside) & BuildingFlags::NO_WATER,
        0,
        "edge just inside the reach: connected"
    );
    assert_ne!(
        bits(&m, outside) & BuildingFlags::NO_WATER,
        0,
        "edge just outside: not"
    );
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

    // Out of reach of the pipe (75 m): it gives nothing.
    let mut b2 = BuildingGrid::new();
    let far = add(&mut b2, &db, "tower", 100.0, 75.0);
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

/// T-110: a treatment plant (a Sewage source standing on land beside a road, far from any
/// water) serves the houses of its pipe component, but only while the component holds a
/// water source — a pump or a tower.
#[test]
fn a_treatment_plant_works_only_with_a_water_source_in_its_component() {
    let db = db();
    let mut g = UtilityGraph::new();
    line(&mut g, UtilityNet::Pipe, 0.0, 200.0);
    let build = |source: Option<&str>| {
        let mut b = BuildingGrid::new();
        if let Some(s) = source {
            add(&mut b, &db, s, 0.0, 10.0);
        }
        add(&mut b, &db, "treatment", 150.0, 10.0);
        let h = add(&mut b, &db, "house", 100.0, 10.0);
        (compute_supply(&db, &b, &g), h)
    };
    for source in ["pump", "tower"] {
        let (m, h) = build(Some(source));
        assert_eq!(bits(&m, h) & BuildingFlags::NO_SEWAGE, 0, "with a {source}");
    }
    let (m, h) = build(None);
    assert_ne!(bits(&m, h) & BuildingFlags::NO_SEWAGE, 0, "no water source");
}

/// A building of `cells` x `cells` (a lot, or free-standing) centred at `(x, z)`.
fn add_sized(
    grid: &mut BuildingGrid,
    db: &ContentDb,
    def: &str,
    cells: u8,
    free: bool,
    x: f32,
    z: f32,
) -> BuildingId {
    let id = add(grid, db, def, x, z);
    let b = grid.get_mut(id).unwrap();
    b.footprint = if free {
        b.access = None;
        cp_building::Footprint::Free { w: cells, d: cells }
    } else {
        cp_building::Footprint::Lot(Lot {
            block: Id::new(0, 0),
            col: 0,
            w: cells,
            d: cells,
        })
    };
    id
}

/// BUG-036: a 3x3 house right beside a free-standing 2x2 turbine gets current without a line, a
/// chain of three neighbouring houses from the turbine is served whole, and a house 80 m
/// (edge to edge) away is not.
#[test]
fn power_passes_between_neighbouring_footprints_not_between_far_ones() {
    let db = db();
    let g = UtilityGraph::new();
    let mut b = BuildingGrid::new();
    let turbine = add_sized(&mut b, &db, "bigplant", 2, true, 0.0, 0.0);
    // Centres 20 m apart: the edges are 4 m apart, the centres were never within 30 m of each
    // other for lots like these.
    let h1 = add_sized(&mut b, &db, "house", 3, false, 20.0, 0.0);
    let h2 = add_sized(&mut b, &db, "house", 3, false, 44.0, 0.0);
    let h3 = add_sized(&mut b, &db, "house", 3, false, 68.0, 0.0);
    let far = add_sized(&mut b, &db, "house", 3, false, 0.0, 100.0);
    let m = compute_supply(&db, &b, &g);
    for (name, id) in [("turbine", turbine), ("h1", h1), ("h2", h2), ("h3", h3)] {
        assert_eq!(bits(&m, id) & BuildingFlags::NO_POWER, 0, "{name}");
    }
    assert_ne!(
        bits(&m, far) & BuildingFlags::NO_POWER,
        0,
        "edge gap of 80 m, no chain"
    );
}

/// The user's case (BUG-036): a plant with a block of houses round it and no line at all powers
/// the houses on both sides of the streets; a house across a 100 m field only gets current once
/// a line bridges the field.
#[test]
fn a_plant_powers_a_block_without_a_line_and_a_line_bridges_a_field() {
    let db = db();
    let mut b = BuildingGrid::new();
    let plant = add_sized(&mut b, &db, "bigplant", 4, true, 0.0, 0.0);
    // Two rows of three houses on either side of a 28 m street (the widest zonable road).
    let mut block = Vec::new();
    for z in [30.0, 82.0] {
        for x in [20.0, 44.0, 68.0] {
            block.push(add_sized(&mut b, &db, "house", 3, false, x, z));
        }
    }
    // 100 m of field beyond the block.
    let across = add_sized(&mut b, &db, "house", 3, false, 0.0, 206.0);
    let mut g = UtilityGraph::new();
    let m = compute_supply(&db, &b, &g);
    assert_eq!(bits(&m, plant) & BuildingFlags::NO_POWER, 0);
    for (i, h) in block.iter().enumerate() {
        assert_eq!(bits(&m, *h) & BuildingFlags::NO_POWER, 0, "house {i}");
    }
    assert_ne!(
        bits(&m, across) & BuildingFlags::NO_POWER,
        0,
        "across the field"
    );

    // A line along the field joins the far house to the plant's component.
    let p = |z: f32| Vec3::new(5000.0, 0.0, 5000.0 + z);
    let (a, c) = (
        g.add_node(UtilityNet::Power, p(0.0)),
        g.add_node(UtilityNet::Power, p(206.0)),
    );
    g.add_link(a, c, CubicBezier3::line(p(0.0), p(206.0)))
        .unwrap();
    let m = compute_supply(&db, &b, &g);
    assert_eq!(bits(&m, across) & BuildingFlags::NO_POWER, 0, "bridged");
}

/// T-181: power crosses the default `power_gap_m` (64 m between footprint edges), not more, and
/// the content decides: a wider gap in `balance.ron` powers the house that was out of reach.
#[test]
fn the_power_gap_comes_from_the_content() {
    // 1x1 lots are 8 m wide: a centre distance of 68 m is an edge gap of 60 m.
    let powered = |db: &ContentDb, x: f32| {
        let mut b = BuildingGrid::new();
        add(&mut b, db, "plant", 0.0, 0.0);
        let h = add(&mut b, db, "house", x, 0.0);
        bits(&compute_supply(db, &b, &UtilityGraph::new()), h) & BuildingFlags::NO_POWER == 0
    };
    let db = db();
    assert!(powered(&db, 68.0), "gap of 60 m: power passes");
    assert!(!powered(&db, 78.0), "gap of 70 m: it does not");
    let wide = db_with("networks: (power_gap_m: 80.0)");
    assert!(powered(&wide, 78.0), "the content widened the gap");
    let narrow = db_with("networks: (power_gap_m: 20.0)");
    assert!(!powered(&narrow, 68.0), "and narrowed it");
    assert_ne!(db.content_hash(), wide.content_hash());
}

/// T-181: a power line picks up a house 75 m away (`line_reach_m` 80); a pipe only 64 m.
#[test]
fn the_line_and_pipe_reach_come_from_the_content() {
    let db = db();
    let mut b = BuildingGrid::new();
    let mut g = UtilityGraph::new();
    line(&mut g, UtilityNet::Power, 0.0, 100.0);
    line(&mut g, UtilityNet::Pipe, 0.0, 100.0);
    add(&mut b, &db, "plant", 0.0, 10.0);
    add(&mut b, &db, "pump", 0.0, 10.0);
    let near = add(&mut b, &db, "house", 50.0, 75.0);
    let far = add(&mut b, &db, "house", 50.0, -95.0);
    let m = compute_supply(&db, &b, &g);
    assert_eq!(
        bits(&m, near) & BuildingFlags::NO_POWER,
        0,
        "line within 80 m"
    );
    assert_ne!(
        bits(&m, far) & BuildingFlags::NO_POWER,
        0,
        "line beyond 80 m"
    );
    assert_ne!(
        bits(&m, near) & BuildingFlags::NO_WATER,
        0,
        "75 m is beyond pipe_reach_m 64"
    );
    let wide = db_with("networks: (pipe_reach_m: 90.0)");
    let m = compute_supply(&wide, &b, &g);
    assert_eq!(
        bits(&m, near) & BuildingFlags::NO_WATER,
        0,
        "a longer pipe reach"
    );
}

/// BUG-042: a tower within reach of a spur it is drawn to and of the street pipe the houses are
/// on (the two not joined by a node) serves both: one network, the houses have water. The same
/// goes for a plant between two power lines.
#[test]
fn a_source_joins_every_network_of_its_net_within_reach() {
    let db = db();
    let mut b = BuildingGrid::new();
    let mut g = UtilityGraph::new();
    // The street pipe, and a spur 30 m north of it that stops short of it.
    line(&mut g, UtilityNet::Pipe, 0.0, 200.0);
    let p = |x: f32, z: f32| Vec3::new(5000.0 + x, 0.0, 5000.0 + z);
    let (a, c) = (
        g.add_node(UtilityNet::Pipe, p(100.0, -30.0)),
        g.add_node(UtilityNet::Pipe, p(160.0, -30.0)),
    );
    g.add_link(a, c, CubicBezier3::line(p(100.0, -30.0), p(160.0, -30.0)))
        .unwrap();
    let tower = add(&mut b, &db, "tower", 130.0, -45.0);
    let house = add(&mut b, &db, "house", 40.0, 15.0);
    let missing = compute_supply(&db, &b, &g);
    assert_eq!(bits(&missing, house) & BuildingFlags::NO_WATER, 0);
    assert_eq!(bits(&missing, tower) & BuildingFlags::NO_WATER, 0);

    // Power: a plant between two lines that do not touch.
    let mut g = UtilityGraph::new();
    line(&mut g, UtilityNet::Power, 0.0, 100.0);
    let (a, c) = (
        g.add_node(UtilityNet::Power, p(100.0, -30.0)),
        g.add_node(UtilityNet::Power, p(160.0, -30.0)),
    );
    g.add_link(a, c, CubicBezier3::line(p(100.0, -30.0), p(160.0, -30.0)))
        .unwrap();
    let mut b = BuildingGrid::new();
    add(&mut b, &db, "plant", 130.0, -45.0);
    let far = add(&mut b, &db, "house", 10.0, 15.0);
    let missing = compute_supply(&db, &b, &g);
    assert_eq!(bits(&missing, far) & BuildingFlags::NO_POWER, 0);
}

#[test]
fn power_saving_lets_the_same_plant_carry_more_houses() {
    use cp_content::PolicyKind;
    use cp_district::{CityPolicies, DistrictMap, Districts, Policies};

    // Houses need 10 of power, the plant gives 25: two fit. With 30 % saved they need 7: three fit.
    let mut sources = vec![
        (
            "style/p.ron".to_string(),
            cp_content::TEST_STYLE_RON.to_string(),
        ),
        building_src("house", None, "{Power: 10.0}"),
        building_src("plant", Some("plant"), "{}"),
        service_src("plant", "Power", 25),
    ];
    sources.push(cp_content::test_policy_source("PowerSaving", 0.3));
    let db = ContentDb::from_sources(sources).unwrap_or_else(|e| panic!("{e}"));
    let mut b = BuildingGrid::new();
    let mut g = UtilityGraph::new();
    line(&mut g, UtilityNet::Power, 0.0, 200.0);
    add(&mut b, &db, "plant", 0.0, 10.0);
    let houses: Vec<_> = (0..4)
        .map(|i| add(&mut b, &db, "house", 20.0 + 30.0 * i as f32, 15.0))
        .collect();
    let powered = |m: &[u8]| {
        houses
            .iter()
            .filter(|h| m[h.index as usize] & BuildingFlags::NO_POWER == 0)
            .count()
    };
    let plain = compute_supply_cached(&db, &b, &g, &mut AttachCache::default());
    assert_eq!(powered(&plain), 2);

    let (map, districts) = (DistrictMap::default(), Districts::default());
    let mut city = CityPolicies::default();
    let with = |city: &CityPolicies| {
        let p = Policies {
            map: &map,
            districts: &districts,
            city,
        };
        compute_supply_with(&db, &b, &g, &mut AttachCache::default(), Some(&p))
    };
    assert_eq!(powered(&with(&city)), 2, "policy off");
    city.mask = PolicyKind::PowerSaving.bit();
    assert_eq!(powered(&with(&city)), 3, "policy on");
}

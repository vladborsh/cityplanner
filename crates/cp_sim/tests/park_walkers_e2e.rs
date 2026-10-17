//! T-224: tourists who pick a park come to its gate and pay, and while a park has visitors a
//! few pedestrians walk its paths from the gate to an attraction and back — never more than
//! `walkers_per_park`.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingFlags, BuildingGrid, BuildingState, Footprint, Lot,
};
use cp_content::{ContentDb, ParkKind};
use cp_core::curve::CubicBezier3;
use cp_core::{Id, Tick};
use cp_district::{AreaId, AreaKind, DistrictCommand};
use cp_net::{Elevation, NetGraph, NodeKind};
use cp_park::{ParkLedger, ParkState, ParkVisits, Visit};
use cp_sim::{SimCommand, SimRunner};
use cp_traffic::Agents;

fn content_with_balance(balance: &str) -> Arc<ContentDb> {
    fn walk(dir: &Path, out: &mut Vec<(String, String)>, balance: &str) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            if path.is_dir() {
                walk(&path, out, balance);
            } else if path.extension().is_some_and(|x| x == "ron") {
                let mut text = std::fs::read_to_string(&path).unwrap();
                if path.file_name().is_some_and(|n| n == "balance.ron") {
                    text = format!("Balance(({balance}))");
                }
                out.push((path.display().to_string(), text));
            }
        }
    }
    let mut sources = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content"),
        &mut sources,
        balance,
    );
    Arc::new(ContentDb::from_sources(sources).unwrap())
}

fn insert(sim: &mut SimRunner, c: &ContentDb, id: &str, pos: Vec2, access: Option<AccessPoint>) {
    let def = c.building_id(id).unwrap();
    let d = c.building(def);
    let footprint = if access.is_some() {
        Footprint::Lot(Lot {
            block: Id::new(0, 0),
            col: 0,
            w: d.lot.0,
            d: d.lot.1,
        })
    } else {
        Footprint::Free {
            w: d.lot.0,
            d: d.lot.1,
        }
    };
    sim.world_mut()
        .resource_mut::<BuildingGrid>()
        .insert(Building {
            def,
            footprint,
            level: 1,
            state: BuildingState::Active,
            ready_at: Tick(0),
            pos,
            rot: 0.0,
            access,
            flags: BuildingFlags::default(),
            level_progress: 0.0,
        });
}

/// A 1 km street from an `Outside` node at its west end, a footpath to the north from the
/// middle, three shops, and a city-park area with a gate on the street and an attraction at
/// the end of the path.
fn world(c: &Arc<ContentDb>) -> (SimRunner, AreaId) {
    let road = c.road_id("basic_2l").unwrap();
    let path = c.road_id("park_path").unwrap();
    let (rd, pd) = (c.road(road).clone(), c.road(path).clone());
    let mut sim = SimRunner::new(7);
    sim.set_content(Arc::clone(c));
    let (a, b, p) = (
        Vec3::new(5000.0, 0.0, 5000.0),
        Vec3::new(5500.0, 0.0, 5000.0),
        Vec3::new(5500.0, 0.0, 5100.0),
    );
    let seg = {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        g.add_node(a, NodeKind::Outside, Elevation::Ground);
        let s1 = g
            .add_segment(&rd, road, CubicBezier3::line(a, b), Elevation::Ground)
            .unwrap();
        g.add_segment(
            &rd,
            road,
            CubicBezier3::line(b, Vec3::new(6000.0, 0.0, 5000.0)),
            Elevation::Ground,
        )
        .unwrap();
        g.add_segment(&pd, path, CubicBezier3::line(b, p), Elevation::Ground)
            .unwrap();
        s1
    };
    for (i, t) in [0.2, 0.4, 0.6].into_iter().enumerate() {
        insert(
            &mut sim,
            c,
            "com_high_1",
            Vec2::new(5000.0 + 500.0 * t, 5010.0 + i as f32),
            Some(AccessPoint { segment: seg, t }),
        );
    }
    let area = {
        let cell = |x: f32, z: f32| cp_district::DistrictMap::cell_of(Vec2::new(x, z)).unwrap();
        let cells = (0..8)
            .flat_map(|i| (0..8).map(move |j| (5440.0 + 32.0 * i as f32, 5000.0 + 32.0 * j as f32)))
            .map(|(x, z)| cell(x, z))
            .collect();
        for cmd in [
            DistrictCommand::CreateArea {
                kind: AreaKind::Park(ParkKind::CityPark),
                name: "Park".into(),
            },
            DistrictCommand::PaintArea {
                cells,
                area: AreaId(1),
            },
        ] {
            sim.push_command(SimCommand::District(cmd), sim.tick());
            sim.tick_once();
        }
        AreaId(1)
    };
    insert(
        &mut sim,
        c,
        "city_park_gate",
        Vec2::new(5470.0, 5012.0),
        Some(AccessPoint {
            segment: seg,
            t: 0.94,
        }),
    );
    insert(
        &mut sim,
        c,
        "city_park_attr_1",
        Vec2::new(5500.0, 5100.0),
        None,
    );
    insert(
        &mut sim,
        c,
        "city_park_attr_2",
        Vec2::new(5560.0, 5100.0),
        None,
    );
    // The hourly count fills the leisure targets.
    for _ in 0..200 {
        sim.tick_once();
    }
    (sim, area)
}

#[test]
fn tourists_with_a_share_come_to_the_gate_and_pay() {
    let c = content_with_balance(
        "tourist_leisure_share: 1.0, tourism_base: 600.0, leisure_chance_per_day: 0.0",
    );
    let (mut sim, area) = world(&c);
    sim.world_mut()
        .resource_mut::<ParkState>()
        .areas
        .get_mut(&area.0)
        .unwrap()
        .fee = 4.0;
    for _ in 0..3 * 4096 {
        sim.tick_once();
    }
    let st = sim.world().resource::<ParkState>().areas[&area.0];
    assert!(st.visitors_week > 0, "tourists visited: {st:?}");
    assert!(sim.world().resource::<ParkLedger>().fees.0 > 0);
}

#[test]
fn walkers_walk_the_path_while_there_are_visitors_and_never_exceed_the_cap() {
    let c = content_with_balance("walkers_per_park: 3");
    let (mut sim, area) = world(&c);
    sim.world_mut()
        .resource_mut::<ParkVisits>()
        .inside
        .extend((0..10).map(|_| Visit {
            area: area.0,
            leave_at: Tick(u64::MAX / 2),
        }));
    let (mut most, mut pedestrians) = (0, 0);
    for _ in 0..1500 {
        sim.tick_once();
        most = most.max(sim.world().resource::<ParkVisits>().walkers.len());
        pedestrians = pedestrians.max(sim.world().resource::<Agents>().pedestrian_count());
    }
    assert!(most >= 1, "someone walks the park");
    assert!(most <= 3, "at most walkers_per_park: {most}");
    assert!(pedestrians >= 1, "a pedestrian is on the way");

    // The visitors go: the walkers finish and no new ones start.
    sim.world_mut().resource_mut::<ParkVisits>().inside.clear();
    for _ in 0..4000 {
        sim.tick_once();
    }
    assert!(sim.world().resource::<ParkVisits>().walkers.is_empty());
}

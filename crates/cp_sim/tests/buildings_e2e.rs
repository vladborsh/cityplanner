//! T-028 acceptance: zoned blocks fill with buildings of every kind, lots never overlap,
//! every building has an access point, construction takes exactly `build_hours`, and the
//! whole thing is deterministic.

use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingGrid, BuildingState, Demand, build_ticks};
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_sim::SimRunner;
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

/// A 2x2 block lattice of 90 m streets near the map centre, all blocks painted.
fn city(content: &Arc<ContentDb>, seed: u64) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(seed);
    sim.set_content(Arc::clone(content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(3000.0 + x, 0.0, 3000.0 + z);
        for line in 0..3 {
            let o = line as f32 * 90.0;
            for piece in 0..2 {
                let a = piece as f32 * 90.0;
                for (from, to) in [
                    (p(a, o), p(a + 90.0, o)), // horizontal
                    (p(o, a), p(o, a + 90.0)), // vertical
                ] {
                    g.add_segment(&def, road, CubicBezier3::line(from, to), Elevation::Ground)
                        .unwrap();
                }
            }
        }
    }
    sim.tick_once(); // derive junctions and zone blocks
    paint(&mut sim);
    sim
}

/// Paints block `i` with a cycle of the four kinds (residential, commercial, industry, office).
fn paint(sim: &mut SimRunner) {
    let kinds = [
        ZoneType::ResLow,
        ZoneType::ComLow,
        ZoneType::Industrial,
        ZoneType::Office,
        ZoneType::ResHigh,
        ZoneType::ComHigh,
    ];
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

fn run_days(sim: &mut SimRunner, days: u64) {
    for _ in 0..days * TICKS_PER_DAY {
        sim.tick_once();
    }
}

#[test]
fn zoned_blocks_fill_with_all_four_kinds_without_overlap() {
    let content = content();
    let mut sim = city(&content, 5);
    assert!(sim.world().resource::<ZoneGrid>().block_count() >= 12);
    run_days(&mut sim, 3);

    let buildings = sim.world().resource::<BuildingGrid>();
    let grid = sim.world().resource::<ZoneGrid>();
    let net = sim.world().resource::<NetGraph>();
    assert!(buildings.len() > 20, "{} buildings", buildings.len());

    // Residential, commercial, industrial and office all appear.
    let mut seen = std::collections::BTreeSet::new();
    for (_, b) in buildings.iter() {
        seen.insert(content.building(b.def).zone);
    }
    let family = |z: ZoneType| match z {
        ZoneType::ResLow | ZoneType::ResHigh => 0,
        ZoneType::ComLow | ZoneType::ComHigh => 1,
        ZoneType::Industrial => 2,
        _ => 3,
    };
    let kinds: std::collections::BTreeSet<_> = seen.iter().map(|z| family(*z)).collect();
    assert_eq!(kinds.len(), 4, "{seen:?}");

    // No two lots share a cell, and no two buildings' cell footprints interpenetrate.
    let mut cells: Vec<(ZoneCellRef, usize)> = Vec::new();
    for (n, (_, b)) in buildings.iter().enumerate() {
        for c in b.lot().expect("a lot").cells() {
            assert!(grid.is_occupied(c), "{c:?} is not marked occupied");
            assert!(!grid.is_valid(c));
            cells.push((c, n));
        }
    }
    let mut sorted: Vec<_> = cells.iter().map(|(c, _)| *c).collect();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), cells.len(), "a cell belongs to two lots");
    for i in 0..cells.len() {
        let a = grid.cell_footprint(cells[i].0).unwrap();
        for (cj, nj) in &cells[i + 1..] {
            if *nj != cells[i].1 {
                let b = grid.cell_footprint(*cj).unwrap();
                assert!(
                    a.penetration(&b) <= cp_zone::PENETRATION_EPS + 1e-3,
                    "buildings {} and {nj} overlap",
                    cells[i].1
                );
            }
        }
    }

    // Every building touches the road (row 0), is painted with its def's zone and has an
    // access point on an existing segment close to the lot.
    for (_, b) in buildings.iter() {
        let def = content.building(b.def);
        assert_eq!(
            (b.lot().expect("a lot").w, b.lot().expect("a lot").d),
            def.lot
        );
        let first = grid.block(b.lot().expect("a lot").block).unwrap();
        assert_eq!(
            first.zones[usize::from(b.lot().expect("a lot").col * 4)],
            def.zone
        );
        let seg = net
            .segment(b.access().expect("an access point").segment)
            .expect("access segment exists");
        assert!((0.0..=1.0).contains(&b.access().expect("an access point").t));
        let at = seg.curve.point(b.access().expect("an access point").t);
        let d = bevy_math::Vec2::new(at.x - b.pos.x, at.z - b.pos.y).length();
        assert!(d < 45.0, "access point {d} m from the lot");
        assert_eq!(def.level, 1);
        assert!(def.service.is_none());
    }
}

#[test]
fn construction_takes_exactly_build_hours() {
    let content = content();
    let mut sim = city(&content, 5);
    // Run until the first building appears.
    let mut spawned = None;
    for _ in 0..2000 {
        sim.tick_once();
        if !sim.world().resource::<BuildingGrid>().is_empty() {
            spawned = Some(
                sim.world()
                    .resource::<BuildingGrid>()
                    .iter()
                    .next()
                    .unwrap()
                    .0,
            );
            break;
        }
    }
    let id = spawned.expect("something spawned within 2000 ticks");
    let b = sim
        .world()
        .resource::<BuildingGrid>()
        .get(id)
        .unwrap()
        .clone();
    assert_eq!(b.state, BuildingState::UnderConstruction);
    // Spawned during the tick before the current one.
    let spawn_tick = sim.tick().0 - 1;
    let def = content.building(b.def);
    assert_eq!(b.ready_at.0, spawn_tick + build_ticks(def));

    // Still under construction after the schedule of tick ready_at - 1 ...
    while sim.tick().0 < b.ready_at.0 {
        sim.tick_once();
    }
    assert_eq!(sim.tick().0, b.ready_at.0);
    let state = |sim: &SimRunner| {
        sim.world()
            .resource::<BuildingGrid>()
            .get(id)
            .unwrap()
            .state
    };
    assert_eq!(state(&sim), BuildingState::UnderConstruction);
    // ... and active once the schedule of tick ready_at has run.
    sim.tick_once();
    assert_eq!(state(&sim), BuildingState::Active);
}

#[test]
fn no_demand_means_no_buildings() {
    let content = content();
    let mut sim = city(&content, 5);
    *sim.world_mut().resource_mut::<Demand>() = Demand::fixed(0.0);
    run_days(&mut sim, 1);
    assert!(sim.world().resource::<BuildingGrid>().is_empty());
}

#[test]
fn same_seed_same_city_different_seed_different_city() {
    let content = content();
    let (mut a, mut b, mut c) = (city(&content, 5), city(&content, 5), city(&content, 6));
    for sim in [&mut a, &mut b, &mut c] {
        run_days(sim, 2);
    }
    assert_eq!(a.checksum(), b.checksum());
    assert_eq!(
        a.world().resource::<BuildingGrid>(),
        b.world().resource::<BuildingGrid>()
    );
    assert_ne!(a.checksum(), c.checksum());
}

#[test]
fn buildings_are_part_of_the_checksum() {
    let content = content();
    let mut a = city(&content, 5);
    let before = a.checksum();
    run_days(&mut a, 1);
    assert!(!a.world().resource::<BuildingGrid>().is_empty());
    assert_ne!(a.checksum(), before);
}

#[test]
fn the_arena_gauge_reports_buildings() {
    let content = content();
    let mut sim = city(&content, 5);
    run_days(&mut sim, 1);
    let stats = sim.view().stats;
    let fill = stats.arenas.iter().find(|a| a.name == "buildings").unwrap();
    assert_eq!(
        fill.used as usize,
        sim.world().resource::<BuildingGrid>().len()
    );
    assert!(fill.used > 0);
}

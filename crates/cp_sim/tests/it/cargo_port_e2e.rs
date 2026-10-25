//! T-217: the cargo port. It stands only on the shore; exports go through it when it is nearer
//! than the entry highway; a ship comes by timer, stands at the berth and leaves with the exports,
//! which are paid when it leaves; replays agree.

use std::path::Path;
use std::sync::Arc;

use bevy_math::{Vec2, Vec3};
use cp_building::{
    AccessPoint, Building, BuildingError, BuildingFlags, BuildingGrid, BuildingId, BuildingState,
    Footprint, Lot, validate_placement,
};
use cp_citizen::CitizenGrid;
use cp_content::{CargoKind, ContentDb, ResourceKind};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_core::{Cost, Id, Tick};
use cp_district::{Area, AreaId, AreaKind, AreaMap, Areas};
use cp_industry::{IndustryLedger, IndustryState};
use cp_net::{Elevation, NetGraph, NodeKind};
use cp_sim::SimRunner;
use cp_terrain::{GRID, MapMeta, RES_GRID, ResourceMap, Terrain};
use cp_transit::CargoHubs;
use cp_zone::ZoneGrid;

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

struct Town {
    sim: SimRunner,
    db: Arc<ContentDb>,
    seg: cp_net::SegmentId,
}

/// A 1 km street along z = 5 (an `Outside` node at x = 0 when `outside`), a forestry area over
/// it with its Main building at x = 40, and forest under the whole street.
fn town(outside: bool) -> Town {
    let db = content();
    let mut sim = SimRunner::new(3);
    sim.set_content(Arc::clone(&db));
    let meta = MapMeta {
        name: "flat".into(),
        sea_level: 0.0,
        start_tile: (0, 0),
        external_connections: vec![],
        entry_road: None,
        entry_rail: None,
    };
    let px = (0..RES_GRID * RES_GRID)
        .map(|i| {
            if i / RES_GRID == 0 {
                [0, 255, 0, 0]
            } else {
                [0; 4]
            }
        })
        .collect();
    sim.set_terrain(Arc::new(
        Terrain::from_samples(meta, &vec![1000; GRID * GRID], &vec![0; GRID * GRID])
            .unwrap()
            .with_resources(ResourceMap::from_pixels(px).unwrap()),
    ));
    let road = db.road_id("basic_2l").unwrap();
    let def = db.road(road).clone();
    if outside {
        sim.world_mut().resource_mut::<NetGraph>().add_node(
            Vec3::ZERO,
            NodeKind::Outside,
            Elevation::Ground,
        );
    }
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
    {
        let w = sim.world_mut();
        w.resource_mut::<Areas>().list.push(Some(Area {
            name: "Forest 1".into(),
            kind: AreaKind::Industry(ResourceKind::Forestry),
            level: 1,
            xp: 0,
        }));
        let mut map = w.resource_mut::<AreaMap>();
        for c in 0..32 {
            map.set((c, 0), Some(AreaId(1)));
        }
    }
    let mut t = Town { sim, db, seg };
    t.put("forest_main", 40.0);
    t.sim.tick_once();
    t
}

impl Town {
    fn put(&mut self, name: &str, at: f32) -> BuildingId {
        let seg = self.seg;
        self.sim
            .world_mut()
            .resource_mut::<BuildingGrid>()
            .insert(Building {
                def: self.db.building_id(name).unwrap(),
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
    }

    /// Tops the building's workers of education 0 up to `n`.
    fn staff(&mut self, b: BuildingId, n: usize) {
        let mut g = self.sim.world_mut().resource_mut::<CitizenGrid>();
        let mut left = n.saturating_sub(usize::from(g.jobs_filled(b, 0)));
        while left > 0 {
            let k = left.min(4);
            left -= k;
            let hid = g.add_household(b, 0, Tick(0), &vec![(9000, 0, false); k]);
            let members: Vec<_> = g
                .household(hid)
                .unwrap()
                .members
                .iter()
                .flatten()
                .copied()
                .collect();
            for m in members {
                g.set_job(m, b, 0);
            }
        }
    }

    fn seed(&mut self, b: BuildingId, c: CargoKind, v: f32) {
        self.sim
            .world_mut()
            .resource_mut::<IndustryState>()
            .stock
            .insert(b, vec![(c, v)]);
    }

    fn ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.sim.tick_once();
        }
    }
}

impl Town {
    fn hub(&self, b: BuildingId) -> Option<cp_transit::Hub> {
        self.sim
            .world()
            .resource::<CargoHubs>()
            .hubs
            .get(&b)
            .cloned()
    }

    fn ledger(&self) -> IndustryLedger {
        *self.sim.world().resource::<IndustryLedger>()
    }
}

#[test]
fn a_port_needs_water_within_16_m() {
    let content = content();
    let port = content.building_id("cargo_port").unwrap();
    // Flatlands has no water: refused wherever the cursor is.
    let mut t = town(false);
    t.sim.tick_once();
    let view = t.sim.view();
    let zones = t.sim.world().resource::<ZoneGrid>();
    let mut tried = 0;
    for x in (100..900).step_by(20) {
        let r = validate_placement(
            &content,
            zones,
            &view.net,
            None,
            None,
            port,
            Vec2::new(x as f32, 12.0),
            0.0,
        );
        if r != Err(BuildingError::NoRoad) {
            assert_eq!(r.err(), Some(BuildingError::NeedsWater), "x={x}");
            tried += 1;
        }
    }
    assert!(tried > 0, "some spot got as far as the water check");
}

#[test]
fn a_port_can_be_placed_on_the_shore_of_a_real_map() {
    let root = env!("CARGO_MANIFEST_DIR");
    let terrain =
        Arc::new(Terrain::load(&Path::new(root).join("../../assets/maps/river_valley")).unwrap());
    let content = content();
    let port = content.building_id("cargo_port").unwrap();
    let road = content.road_id("basic_2l").unwrap();
    // Search the shore for a road stretch along which some 4x4 lot fits.
    let mut found = false;
    let mut berths: Vec<(Vec2, f32)> = Vec::new();
    'outer: for zi in 0..40 {
        for xi in 0..40 {
            let (x, z) = (300.0 + xi as f32 * 240.0, 300.0 + zi as f32 * 240.0);
            let mut sim = SimRunner::new(5);
            sim.set_terrain(Arc::clone(&terrain));
            sim.set_content(Arc::clone(&content));
            if (0..=20).any(|i| terrain.is_water(Vec2::new(x + i as f32 * 8.0, z))) {
                continue;
            }
            if sim
                .world_mut()
                .resource_mut::<NetGraph>()
                .add_segment(
                    &content.road(road).clone(),
                    road,
                    CubicBezier3::line(Vec3::new(x, 0.0, z), Vec3::new(x + 160.0, 0.0, z)),
                    Elevation::Ground,
                )
                .is_err()
            {
                continue;
            }
            sim.tick_once();
            sim.tick_once();
            let view = sim.view();
            let zones = sim.world().resource::<ZoneGrid>();
            for (id, block) in zones.blocks() {
                for cell in 0..block.cell_count() {
                    let r = cp_zone::ZoneCellRef {
                        block: id,
                        cell: cell as u16,
                    };
                    let Some(f) = zones.cell_footprint(r) else {
                        continue;
                    };
                    if let Ok(plan) = validate_placement(
                        &content,
                        zones,
                        &view.net,
                        Some(&terrain),
                        None,
                        port,
                        f.c,
                        0.0,
                    ) {
                        found = true;
                        berths.push((plan.pos, plan.rot));
                        if berths.len() >= 6 {
                            break 'outer;
                        }
                    }
                }
            }
        }
    }
    assert!(found, "no spot for the port on the shore of river_valley");
    // BUG-071: the ship's line from each of those berths keeps to the water.
    for &(pos, rot) in &berths {
        let (_, _, points) = cp_transit::ship_route(pos, rot, |p| terrain.is_water(p));
        assert!(
            points.len() > 3,
            "{pos:?}: a line of {} points",
            points.len()
        );
        assert!(
            points.iter().skip(1).all(|&p| terrain.is_water(p)),
            "{pos:?}: {points:?}"
        );
        assert_eq!(
            cp_transit::ship_route(pos, rot, |p| terrain.is_water(p)).2,
            points
        );
    }
}

/// The saw at 100, a port (no railway) at 130 and a highway entry at 0.
fn harbour(logs: f32) -> (Town, BuildingId, BuildingId) {
    let mut t = town(true);
    let saw = t.put("forest_extractor_s", 100.0);
    t.staff(saw, 8);
    t.seed(saw, CargoKind::Logs, logs);
    let port = t.put("cargo_port", 130.0);
    t.sim.tick_once();
    (t, saw, port)
}

#[test]
fn exports_go_through_a_port_and_a_ship_arrives_stands_and_leaves_by_timer() {
    let (mut t, _saw, port) = harbour(40.0);
    // The truck drives to the port (30 m) rather than the highway (100 m).
    let mut waited = false;
    for _ in 0..2 * TICKS_PER_DAY {
        t.sim.tick_once();
        if t.hub(port).is_some_and(|h| h.out_total() > 0.0) {
            waited = true;
            break;
        }
    }
    assert!(waited, "the export waits on the quay");
    assert_eq!(
        t.ledger().export_loads,
        0,
        "nothing sold before the ship leaves"
    );
    // The ship sails in (ship_interval_h 24, ship_sail_h 1), stands ship_dwell_h (6) at the
    // berth and sails out; the exports are paid when it leaves the berth.
    let (mut docked, mut leaving, mut gone) = (None, None, None);
    let mut saw_arriving = false;
    for n in 0..3 * TICKS_PER_DAY {
        t.sim.tick_once();
        let h = t.hub(port).unwrap();
        match h.ship.map(|s| s.phase) {
            Some(cp_transit::ShipPhase::Arriving) => {
                if !saw_arriving {
                    // (No water on this map: the line is the berth alone.)
                    let ships = t.sim.view().ships;
                    assert_eq!(ships.len(), 1);
                    assert_eq!(ships[0].points.len(), 1);
                }
                saw_arriving = true;
            }
            Some(cp_transit::ShipPhase::Docked) => {
                docked.get_or_insert(n);
                assert_eq!(t.ledger().export_loads, 0, "the berthed ship has not paid");
                let ships = t.sim.view().ships;
                assert_eq!(ships[0].pos(), ships[0].berth);
            }
            Some(cp_transit::ShipPhase::Leaving) => {
                leaving.get_or_insert(n);
                assert!(t.ledger().export_loads > 0, "paid when it leaves the berth");
            }
            None if leaving.is_some() => {
                gone = Some(n);
                break;
            }
            None => {}
        }
    }
    assert!(saw_arriving, "a ship sailed in");
    let (n0, n1) = (
        docked.expect("it docked"),
        leaving.expect("and left the berth"),
    );
    gone.expect("and sailed out");
    let dwell = (t.db.balance().ship_dwell_h * cp_core::time::TICKS_PER_HOUR) as u64;
    assert!(
        n1 - n0 >= dwell - 1 && n1 - n0 <= dwell + 2,
        "stood {} ticks",
        n1 - n0
    );
    let l = t.ledger();
    assert!(l.export_loads > 0 && l.exports > Cost::ZERO, "{l:?}");
    let h = t.hub(port).unwrap();
    assert_eq!(h.trains, 1);
    assert!(h.out_total() < 40.0);
}

#[test]
fn a_port_without_cargo_sends_no_ship() {
    let (mut t, saw, port) = harbour(0.0);
    t.staff(saw, 0);
    t.sim.world_mut().resource_mut::<BuildingGrid>().remove(saw);
    t.ticks(3 * TICKS_PER_DAY);
    assert!(
        t.hub(port)
            .is_none_or(|h| h.ship.is_none() && h.trains == 0)
    );
}

#[test]
fn ports_are_deterministic() {
    let run = || {
        let (mut t, _, _) = harbour(60.0);
        t.ticks(3 * TICKS_PER_DAY);
        t.sim.checksum()
    };
    assert_eq!(run(), run());
}

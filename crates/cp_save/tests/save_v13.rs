//! Format 13: the building grid saves the ids removed and not yet drained. A save taken on
//! pause right after a `Demolish` resumes exactly like the uninterrupted run, and the
//! committed v12 fixture migrates with an empty list.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::{BuildingCommand, BuildingGrid, BuildingId};
use cp_citizen::CitizenGrid;
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{AgentsV16, BuildingGridV12, SaveDataV12, encode_v12};
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};
use cp_zone::{ZoneCellRef, ZoneGrid};

fn content() -> Arc<ContentDb> {
    Arc::new(
        ContentDb::load_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/content")).unwrap(),
    )
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/saves")
            .join(name),
    )
    .unwrap()
}

#[test]
fn v12_fixture_migrates_with_an_empty_removed_list_and_round_trips() {
    let bytes = fixture("v12-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 12);
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert!(l.data.buildings.removed().is_empty());
    assert!(!l.data.buildings.is_empty(), "the fixture has buildings");

    // The frozen v12 writer reproduces the committed file byte for byte.
    let v12 = SaveDataV12 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: l.data.net.clone(),
        zones: l.data.zones.clone().to_v34(),
        buildings: BuildingGridV12::from(&l.data.buildings),
        citizens: (&l.data.citizens).into(),
        paths: l.data.paths.clone(),
        utilities: l.data.utilities.clone(),
        trips: l.data.trips.clone(),
        agents: AgentsV16::from(&l.data.agents),
    };
    let again = encode_v12(&v12, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(again, bytes, "frozen v12 writer matches the fixture");

    // Re-saved at v13 it round-trips byte for byte.
    let v13 = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(read_header(&v13).unwrap().0.format_version, FORMAT_VERSION);
    let l2 = load(&v13, h.content_hash).unwrap();
    assert_eq!(l2.data, l.data);
    assert_eq!(
        save_with_map(&l2.data, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        v13
    );
}

fn town(content: &Arc<ContentDb>, seed: u64) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(seed);
    sim.set_content(Arc::clone(content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        let p = |x: f32, z: f32| Vec3::new(2000.0 + x, 0.0, 2000.0 + z);
        for line in 0..4 {
            let o = line as f32 * 90.0;
            for piece in 0..3 {
                let a = piece as f32 * 90.0;
                for (from, to) in [(p(a, o), p(a + 90.0, o)), (p(o, a), p(o, a + 90.0))] {
                    g.add_segment(&def, road, CubicBezier3::line(from, to), Elevation::Ground)
                        .unwrap();
                }
            }
        }
    }
    sim.tick_once();
    let kinds = [
        ZoneType::ResHigh,
        ZoneType::ResHigh,
        ZoneType::Industrial,
        ZoneType::ComHigh,
    ];
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
    while sim.world().resource::<CitizenGrid>().citizen_count() < 150 {
        sim.tick_once();
        assert!(sim.tick().0 < 20 * TICKS_PER_DAY);
    }
    for _ in 0..(2 * TICKS_PER_DAY) {
        sim.tick_once();
    }
    sim
}

/// A workplace with workers.
fn busy_workplace(sim: &SimRunner) -> BuildingId {
    let c = sim.world().resource::<CitizenGrid>();
    sim.world()
        .resource::<BuildingGrid>()
        .iter()
        .find(|(id, _)| (0..4).any(|l| c.jobs_filled(*id, l) > 0))
        .map(|(id, _)| id)
        .expect("somebody works somewhere")
}

#[test]
fn a_save_on_pause_right_after_a_demolish_resumes_like_the_continuous_run() {
    let content = content();
    let hash = content.content_hash();
    let mut a = town(&content, 7);
    let work = busy_workplace(&a);
    let jobs: u16 = {
        let c = a.world().resource::<CitizenGrid>();
        (0..4).map(|l| c.jobs_filled(work, l)).sum()
    };
    assert!(jobs > 0);

    // Paused: the command applies at the current tick, the tick does not advance.
    let at = a.tick();
    a.push_command(
        SimCommand::Building(BuildingCommand::Demolish { building: work }),
        at,
    );
    a.apply_due_commands();
    assert_eq!(a.tick(), at);
    assert_eq!(a.world().resource::<BuildingGrid>().removed(), [work]);

    let bytes = save_runner(&a, hash, "none").unwrap();
    let mut b = SimRunner::new(7);
    b.set_content(Arc::clone(&content));
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(b.world().resource::<BuildingGrid>().removed(), [work]);
    assert_eq!(a.checksum(), b.checksum(), "identical right after the load");

    for _ in 0..300 {
        a.tick_once();
        b.tick_once();
    }
    assert!(a.world().resource::<BuildingGrid>().removed().is_empty());
    assert_eq!(a.checksum(), b.checksum(), "same state after 300 ticks");
    let c = b.world().resource::<CitizenGrid>();
    assert!(
        (0..4).all(|l| c.jobs_filled(work, l) == 0),
        "the workers of the demolished building lost their jobs in the loaded run too"
    );
}

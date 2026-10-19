//! Format 16: the land grids and the abandonment timers are saved. The committed v15 fixture
//! migrates with empty grids and no timers, and a save in the middle of a 256-tick cycle, with
//! timers running, resumes exactly like the uninterrupted run.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_economy::{Decay, LandGrids};
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{AgentsV16, NetGraphV54, SaveDataV15, encode_v15};
use cp_save::*;
use cp_sim::SimRunner;
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
fn v15_fixture_migrates_with_empty_grids_and_no_timers_and_round_trips() {
    let bytes = fixture("v15-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 15);
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert_eq!(l.data.land, LandGrids::default());
    assert_eq!(l.data.decay, Decay::default());
    assert!(!l.data.buildings.is_empty(), "the fixture has buildings");
    assert_ne!(
        l.data.demand,
        cp_building::Demand::default(),
        "the v15 demand came along"
    );

    // The frozen v15 writer reproduces the committed file byte for byte.
    let v15 = SaveDataV15 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: NetGraphV54::from(&l.data.net),
        zones: l.data.zones.clone().to_v34(),
        buildings: cp_save::migrate::BuildingGridV18::from(&l.data.buildings),
        citizens: (&l.data.citizens).into(),
        paths: l.data.paths.clone(),
        utilities: l.data.utilities.clone(),
        trips: l.data.trips.clone(),
        agents: AgentsV16::from(&l.data.agents),
        treasury: l.data.treasury,
        economy: (&l.data.economy).into(),
        demand: l.data.demand,
        demand_acc: l.data.demand_acc,
    };
    let again = encode_v15(&v15, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(again, bytes, "frozen v15 writer matches the fixture");

    // Re-saved at the current version it round-trips byte for byte.
    let v16 = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(read_header(&v16).unwrap().0.format_version, FORMAT_VERSION);
    let l2 = load(&v16, h.content_hash).unwrap();
    assert_eq!(l2.data, l.data);
    assert_eq!(
        save_with_map(&l2.data, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        v16
    );
}

fn town(content: &Arc<ContentDb>) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(5);
    sim.set_content(Arc::clone(content));
    {
        let mut g = sim.world_mut().resource_mut::<NetGraph>();
        for (a, b) in [
            ((3000.0, 3000.0), (3090.0, 3000.0)),
            ((3045.0, 2940.0), (3045.0, 3060.0)),
            ((3000.0, 3090.0), (3090.0, 3090.0)),
        ] {
            g.add_segment(
                &def,
                road,
                CubicBezier3::line(Vec3::new(a.0, 0.0, a.1), Vec3::new(b.0, 0.0, b.1)),
                Elevation::Ground,
            )
            .unwrap();
        }
    }
    sim.tick_once();
    let kinds = [ZoneType::ResLow, ZoneType::Industrial, ZoneType::ComLow];
    let mut grid = sim.world_mut().resource_mut::<ZoneGrid>();
    let ids: Vec<_> = grid.blocks().map(|(i, b)| (i, b.cell_count())).collect();
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
    sim
}

#[test]
fn grids_and_timers_survive_a_save_in_the_middle_of_a_cycle() {
    let content = content();
    let hash = content.content_hash();
    let mut a = town(&content);
    // The town has no water or power lines, so every building is in trouble: the timers
    // start with the first pass and the first abandonments come a week later.
    while a.tick().0 < 5 * TICKS_PER_DAY || a.tick().0 % 256 != 100 {
        a.tick_once();
    }
    assert!(
        !a.world().resource::<Decay>().since.is_empty(),
        "timers are running"
    );
    assert_ne!(*a.world().resource::<LandGrids>(), LandGrids::default());

    let bytes = save_runner(&a, hash, "none").unwrap();
    let mut b = SimRunner::new(5);
    b.set_content(Arc::clone(&content));
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(b.world().resource::<Decay>(), a.world().resource::<Decay>());
    assert_eq!(
        b.world().resource::<LandGrids>(),
        a.world().resource::<LandGrids>()
    );
    assert_eq!(a.checksum(), b.checksum(), "identical right after the load");

    // Through the first abandonments (about day 7) and some of what follows.
    for _ in 0..(4 * TICKS_PER_DAY) {
        a.tick_once();
        b.tick_once();
    }
    let abandoned = a
        .world()
        .resource::<cp_building::BuildingGrid>()
        .iter()
        .filter(|(_, bd)| bd.state == cp_building::BuildingState::Abandoned)
        .count();
    assert!(abandoned > 0, "the run covers the first abandonments");
    assert_eq!(a.checksum(), b.checksum(), "same state four days later");
    assert_eq!(b.world().resource::<Decay>(), a.world().resource::<Decay>());
}

//! Format 15: the smoothed demand and the sums of the cycle in progress are saved. The
//! committed v14 fixture migrates with the demand of a new city, and a save in the middle of a
//! demand cycle resumes exactly like the uninterrupted run.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_building::Demand;
use cp_content::{ContentDb, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_DAY;
use cp_economy::DemandAcc;
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{AgentsV16, SaveDataV14, encode_v14};
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
fn v14_fixture_migrates_with_the_demand_of_a_new_city_and_round_trips() {
    let bytes = fixture("v14-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 14);
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert_eq!(l.data.demand, Demand::default());
    assert_eq!(l.data.demand_acc, DemandAcc::default());
    // The v14 economy state came along.
    assert_eq!(
        l.data.economy.taxes.get(ZoneType::ComLow),
        Some(13),
        "the fixture's tax rate"
    );
    assert!(!l.data.buildings.is_empty(), "the fixture has buildings");

    // The frozen v14 writer reproduces the committed file byte for byte.
    let v14 = SaveDataV14 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: l.data.net.clone(),
        zones: l.data.zones.clone(),
        buildings: cp_save::migrate::BuildingGridV18::from(&l.data.buildings),
        citizens: (&l.data.citizens).into(),
        paths: l.data.paths.clone(),
        utilities: l.data.utilities.clone(),
        trips: l.data.trips.clone(),
        agents: AgentsV16::from(&l.data.agents),
        treasury: l.data.treasury,
        economy: (&l.data.economy).into(),
    };
    let again = encode_v14(&v14, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(again, bytes, "frozen v14 writer matches the fixture");

    // Re-saved at the current version it round-trips byte for byte.
    let v15 = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(read_header(&v15).unwrap().0.format_version, FORMAT_VERSION);
    let l2 = load(&v15, h.content_hash).unwrap();
    assert_eq!(l2.data, l.data);
    assert_eq!(
        save_with_map(&l2.data, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        v15
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
fn a_save_in_the_middle_of_a_demand_cycle_resumes_like_the_continuous_run() {
    let content = content();
    let hash = content.content_hash();
    let mut a = town(&content);
    // Past the first days, stopped 17 ticks into a 64-tick cycle.
    while a.tick().0 < 2 * TICKS_PER_DAY || a.tick().0 % 64 != 17 {
        a.tick_once();
    }
    let acc = *a.world().resource::<DemandAcc>();
    assert_eq!(acc.seen, 17, "the cycle is in progress");
    assert_ne!(*a.world().resource::<Demand>(), Demand::default());

    let bytes = save_runner(&a, hash, "none").unwrap();
    let mut b = SimRunner::new(5);
    b.set_content(Arc::clone(&content));
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(*b.world().resource::<DemandAcc>(), acc);
    assert_eq!(
        b.world().resource::<Demand>(),
        a.world().resource::<Demand>()
    );
    assert_eq!(a.checksum(), b.checksum(), "identical right after the load");

    for _ in 0..(TICKS_PER_DAY / 2) {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum(), "same state half a day later");
    assert_eq!(
        b.world().resource::<Demand>(),
        a.world().resource::<Demand>()
    );
}

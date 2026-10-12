//! Format 14: the treasury, tax rates, budgets and last weekly report are saved. The
//! committed v13 fixture migrates with the default economy, and a city that earned and spent
//! money resumes exactly like the uninterrupted run.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec3;
use cp_content::{ContentDb, ServiceKind, ZoneType};
use cp_core::curve::CubicBezier3;
use cp_core::time::TICKS_PER_WEEK;
use cp_core::{Cost, Treasury};
use cp_economy::{EconomyCommand, EconomyState};
use cp_net::{Elevation, NetGraph};
use cp_save::migrate::{AgentsV16, SaveDataV13, encode_v13};
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};

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
fn v13_fixture_migrates_with_the_default_economy_and_round_trips() {
    let bytes = fixture("v13-basic.sav");
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 13);
    let l = load(&bytes, h.content_hash).unwrap();
    assert!(l.warnings.is_empty());
    assert_eq!(l.data.treasury, Treasury::default());
    assert_eq!(l.data.treasury.balance, Cost(7_000_000));
    assert_eq!(l.data.economy, EconomyState::default());
    assert!(!l.data.buildings.is_empty(), "the fixture has buildings");

    // The frozen v13 writer reproduces the committed file byte for byte.
    let v13 = SaveDataV13 {
        tick: l.data.tick,
        rng: l.data.rng.clone(),
        speed: l.data.speed,
        queue: l.data.queue.clone(),
        log: l.data.log.clone(),
        tiles: l.data.tiles,
        net: l.data.net.clone(),
        zones: l.data.zones.clone(),
        buildings: l.data.buildings.clone(),
        citizens: l.data.citizens.clone(),
        paths: l.data.paths.clone(),
        utilities: l.data.utilities.clone(),
        trips: l.data.trips.clone(),
        agents: AgentsV16::from(&l.data.agents),
    };
    let again = encode_v13(&v13, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(again, bytes, "frozen v13 writer matches the fixture");

    // Re-saved at the current version it round-trips byte for byte.
    let v14 = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(read_header(&v14).unwrap().0.format_version, FORMAT_VERSION);
    let l2 = load(&v14, h.content_hash).unwrap();
    assert_eq!(l2.data, l.data);
    assert_eq!(
        save_with_map(&l2.data, h.content_hash, &h.map_id, h.map_hash).unwrap(),
        v14
    );
}

fn city(content: &Arc<ContentDb>) -> SimRunner {
    let road = content.road_id("basic_2l").unwrap();
    let def = content.road(road).clone();
    let mut sim = SimRunner::new(5);
    sim.set_content(Arc::clone(content));
    sim.world_mut()
        .resource_mut::<NetGraph>()
        .add_segment(
            &def,
            road,
            CubicBezier3::line(
                Vec3::new(3000.0, 0.0, 3000.0),
                Vec3::new(3090.0, 0.0, 3000.0),
            ),
            Elevation::Ground,
        )
        .unwrap();
    sim
}

#[test]
fn money_and_settings_survive_a_save_on_pause_and_resume_like_the_continuous_run() {
    let content = content();
    let hash = content.content_hash();
    let mut a = city(&content);
    // Settings change early; the first week settles with them.
    for cmd in [
        EconomyCommand::SetTaxRate {
            zone: ZoneType::ComHigh,
            pct: 17,
        },
        EconomyCommand::SetBudget {
            service: ServiceKind::Roads,
            pct: 70,
        },
    ] {
        a.push_command(SimCommand::Economy(cmd), a.tick());
    }
    while a.tick().0 <= TICKS_PER_WEEK {
        a.tick_once();
    }
    let week = a.world().resource::<EconomyState>().last_week;
    assert_eq!(week.week, 1);
    assert!(week.road_upkeep.0 > 0);
    assert_eq!(
        a.world().resource::<Treasury>().balance,
        Cost(7_000_000 - week.road_upkeep.0)
    );

    // A command applied on pause is saved with its effect.
    let at = a.tick();
    a.push_command(
        SimCommand::Economy(EconomyCommand::SetTaxRate {
            zone: ZoneType::Office,
            pct: 3,
        }),
        at,
    );
    a.apply_due_commands();
    assert_eq!(a.tick(), at);

    let bytes = save_runner(&a, hash, "none").unwrap();
    let mut b = SimRunner::new(5);
    b.set_content(Arc::clone(&content));
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(
        b.world().resource::<Treasury>(),
        a.world().resource::<Treasury>()
    );
    let eb = *b.world().resource::<EconomyState>();
    assert_eq!(eb, *a.world().resource::<EconomyState>());
    assert_eq!(eb.taxes.get(ZoneType::Office), Some(3));
    assert_eq!(eb.taxes.get(ZoneType::ComHigh), Some(17));
    assert_eq!(eb.last_week, week);
    assert_eq!(a.checksum(), b.checksum(), "identical right after the load");

    // Through the next week boundary.
    for _ in 0..TICKS_PER_WEEK {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.world().resource::<EconomyState>().last_week.week, 2);
    assert_eq!(a.checksum(), b.checksum(), "same state a week later");
    assert_eq!(
        a.world().resource::<Treasury>(),
        b.world().resource::<Treasury>()
    );
}

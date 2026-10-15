//! Format 21 (T-131): the district map and records are saved. The v20 fixture (made by the
//! real v20 writer before the change) loads with no districts, and keeps running as recorded
//! on the v20 code; a city with districts survives save/load and replays to the same checksum.

use std::path::Path;
use std::sync::Arc;

use bevy_math::Vec2;
use cp_content::{ContentDb, ZoneType};
use cp_core::SimLimits;
use cp_district::{DistrictCommand, DistrictId, DistrictMap, Districts, district_at};
use cp_save::*;
use cp_sim::{SimCommand, SimRunner};
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn env() -> (Arc<Terrain>, Arc<ContentDb>) {
    (
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap()),
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap()),
    )
}

fn fresh() -> SimRunner {
    let (terrain, content) = env();
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(content);
    sim
}

fn load(sim: &mut SimRunner, bytes: &[u8]) {
    let hash = env().1.content_hash();
    load_into_runner(sim, bytes, hash).unwrap();
}

fn push(sim: &mut SimRunner, c: DistrictCommand) {
    sim.push_command(SimCommand::District(c), sim.tick());
}

#[test]
fn the_v20_fixture_loads_without_districts_and_runs_on() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v20-basic.sav")).unwrap();
    assert_eq!(read_header(&bytes).unwrap().0.format_version, 20);
    let mut sim = fresh();
    load(&mut sim, &bytes);
    assert_eq!(*sim.world().resource::<Districts>(), Districts::default());
    assert_eq!(
        *sim.world().resource::<DistrictMap>(),
        DistrictMap::default()
    );
    for _ in 0..256 {
        sim.tick_once();
    }
    // 290 residents after 256 ticks: recorded on the v20 code before the change.
    assert_eq!(
        sim.world()
            .resource::<cp_citizen::CitizenGrid>()
            .citizen_count(),
        290
    );
    // Re-saving writes the current version.
    let again = save_runner(&sim, env().1.content_hash(), "flatlands").unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
}

#[test]
fn districts_survive_a_save_and_replay_to_the_same_checksum() {
    let mut a = fresh();
    push(
        &mut a,
        DistrictCommand::Create {
            name: "Old town".into(),
        },
    );
    a.tick_once();
    push(
        &mut a,
        DistrictCommand::Paint {
            cells: vec![(3, 4), (4, 4)],
            district: DistrictId(1),
        },
    );
    push(
        &mut a,
        DistrictCommand::SetTax {
            district: DistrictId(1),
            zone: ZoneType::ComLow,
            pct: Some(20),
        },
    );
    for _ in 0..10 {
        a.tick_once();
    }
    let bytes = save_runner(&a, env().1.content_hash(), "flatlands").unwrap();
    let mut b = fresh();
    load(&mut b, &bytes);
    assert_eq!(a.checksum(), b.checksum());
    assert_eq!(
        district_at(b.world().resource::<DistrictMap>(), Vec2::new(100.0, 140.0)),
        Some(DistrictId(1))
    );
    assert_eq!(
        b.world()
            .resource::<Districts>()
            .tax_override(DistrictId(1), ZoneType::ComLow),
        Some(20)
    );
    // Same ticks after the load, same state.
    for _ in 0..10 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum());

    // The command log replays on a fresh runner to the same districts.
    let mut c = fresh();
    for e in a
        .world()
        .resource::<cp_core::CommandLog<SimCommand>>()
        .entries()
    {
        c.push_command(e.command.clone(), e.tick);
    }
    for _ in 0..a.tick().0 {
        c.tick_once();
    }
    assert_eq!(a.checksum(), c.checksum());
}

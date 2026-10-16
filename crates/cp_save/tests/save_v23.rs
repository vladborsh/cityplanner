//! Format 23 (T-145): the `NO_WORKERS` timers are saved. The v22 fixture (made by the real v22
//! writer) loads with none and round-trips on the new version; a hand-set timer survives a
//! save and load.

use std::path::Path;
use std::sync::Arc;

use cp_building::BuildingGrid;
use cp_content::ContentDb;
use cp_core::{SimLimits, Tick};
use cp_economy::WorkerShortage;
use cp_save::*;
use cp_service::{Fire, ServiceProblems, ServiceState};
use cp_sim::SimRunner;
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v22_fixture_loads_with_no_shortage_timers_and_round_trips() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v22-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 22, "made by the v22 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let mut l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.citizens.citizen_count() > 0);
    assert_eq!(l.data.shortage, WorkerShortage::default());
    let id = l.data.buildings.iter().next().unwrap().0;
    l.data.shortage.since.insert(id, Tick(512));
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
    assert_eq!(l2.data.shortage.since.get(&id), Some(&Tick(512)));
}

/// T-142: a save taken while a building burns loads to the same state, and both runs burn it
/// down and demolish it at the same ticks (same checksum all the way).
#[test]
fn a_save_during_a_fire_keeps_the_checksum_through_the_burn_down() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let hash = content.content_hash();
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let runner = || {
        let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
        sim.set_terrain(Arc::clone(&terrain));
        sim.set_content(Arc::clone(&content));
        sim
    };
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v22-basic.sav")).unwrap();
    let mut a = runner();
    load_into_runner(&mut a, &bytes, hash).unwrap();
    for _ in 0..70 {
        a.tick_once();
    }
    // An unattended fire in a building that is not a service building.
    let victim = {
        let g = a.world().resource::<BuildingGrid>();
        g.iter()
            .find(|(_, b)| content.building(b.def).service.is_none())
            .unwrap()
            .0
    };
    let since = a.tick();
    {
        let mut st = a.world_mut().resource_mut::<ServiceState>();
        let s = st.get_mut(victim);
        s.fire = Some(Fire {
            since,
            out_at: None,
            station: None,
        });
        s.problems.set(ServiceProblems::FIRE, true);
    }
    for _ in 0..300 {
        a.tick_once();
    }
    let saved = save_runner(&a, hash, "flatlands").unwrap();
    let mut b = runner();
    load_into_runner(&mut b, &saved, hash).unwrap();
    assert_eq!(a.checksum(), b.checksum());
    for _ in 0..(3 * 4096) {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum());
    let gone = |s: &SimRunner| s.world().resource::<BuildingGrid>().get(victim).is_none();
    assert!(gone(&a) && gone(&b), "burned down and demolished");
}

//! Format 22 (T-135): citizens carry `sick_since` and `school`, the citizen grid its `dead`
//! table, and the service state and coverage are saved. The v21 fixture (made by the real v21
//! writer) loads with all of them empty and round-trips on the new version; hand-set values
//! survive a save and load with the same checksum; a demolished building loses its records.

use std::path::Path;
use std::sync::Arc;

use cp_building::BuildingGrid;
use cp_citizen::CitizenGrid;
use cp_content::ContentDb;
use cp_core::{SimLimits, Tick};
use cp_save::*;
use cp_service::{Fire, ServiceProblems, ServiceState};
use cp_sim::SimRunner;
use cp_terrain::Terrain;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn content() -> Arc<ContentDb> {
    Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap())
}

fn fixture() -> Vec<u8> {
    std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v21-basic.sav")).unwrap()
}

fn runner(content: &Arc<ContentDb>) -> SimRunner {
    let terrain =
        Arc::new(Terrain::load(&Path::new(ROOT).join("../../assets/maps/flatlands")).unwrap());
    let mut sim = SimRunner::with_limits(1, SimLimits::desktop());
    sim.set_terrain(terrain);
    sim.set_content(Arc::clone(content));
    sim
}

#[test]
fn the_v21_fixture_loads_with_empty_service_state_and_round_trips() {
    let bytes = fixture();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 21, "made by the v21 writer");
    let content = content();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.citizens.citizen_count() > 0);
    assert!(l.data.citizens.dead.is_empty());
    assert!(
        l.data
            .citizens
            .citizens()
            .all(|(_, c)| c.sick_since.is_none() && c.school.is_none())
    );
    assert_eq!(l.data.service_state, ServiceState::default());
    assert_eq!(l.data.coverage.generation, 0);
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
}

#[test]
fn hand_set_service_fields_survive_a_save_and_keep_the_checksum() {
    let content = content();
    let hash = content.content_hash();
    let mut a = runner(&content);
    load_into_runner(&mut a, &fixture(), hash).unwrap();
    for _ in 0..70 {
        a.tick_once();
    }
    let (b1, b2) = {
        let g = a.world().resource::<BuildingGrid>();
        let mut ids = g.iter().map(|(id, _)| id);
        (ids.next().unwrap(), ids.next().unwrap())
    };
    {
        let mut st = a.world_mut().resource_mut::<ServiceState>();
        let s = st.get_mut(b1);
        s.garbage = 12;
        s.crime = 3;
        s.fire = Some(Fire {
            since: Tick(50),
            out_at: Some(Tick(60)),
        });
        s.problems.set(ServiceProblems::GARBAGE, true);
        s.problems.set(ServiceProblems::FIRE, true);
        let f = st.facility_mut(b2);
        f.used_today = 4;
        f.stored = 9;
    }
    {
        let mut c = a.world_mut().resource_mut::<CitizenGrid>();
        c.dead.insert(b2, 5);
        c.clear_schools(|_| false);
    }
    let bytes = save_runner(&a, hash, "flatlands").unwrap();
    let mut b = runner(&content);
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(
        a.world().resource::<ServiceState>(),
        b.world().resource::<ServiceState>()
    );
    assert_eq!(b.world().resource::<CitizenGrid>().dead.get(&b2), Some(&5));
    assert_eq!(a.checksum(), b.checksum());
    for _ in 0..100 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum());
}

#[test]
fn a_demolished_building_loses_its_service_records_and_dead() {
    let content = content();
    let hash = content.content_hash();
    let mut a = runner(&content);
    load_into_runner(&mut a, &fixture(), hash).unwrap();
    let victim = a
        .world()
        .resource::<BuildingGrid>()
        .iter()
        .next()
        .unwrap()
        .0;
    a.world_mut()
        .resource_mut::<ServiceState>()
        .get_mut(victim)
        .garbage = 4;
    a.world_mut()
        .resource_mut::<ServiceState>()
        .facility_mut(victim)
        .stored = 2;
    a.world_mut()
        .resource_mut::<CitizenGrid>()
        .dead
        .insert(victim, 3);
    a.world_mut().resource_mut::<BuildingGrid>().remove(victim);
    a.tick_once();
    let st = a.world().resource::<ServiceState>();
    assert!(st.get(victim).is_none() && st.facility(victim).is_none());
    assert!(a.world().resource::<CitizenGrid>().dead.is_empty());
}

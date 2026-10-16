//! Format 29 (T-154): runs between points, kept vehicles. The v28 fixture (made by the real v28
//! writer) loads and round-trips on the new version; a runner saved while a kept vehicle stands
//! at its goal loads to the same checksum and goes on identically.

use std::path::Path;
use std::sync::Arc;

use cp_content::{ContentDb, RoadClass, VehicleKind};
use cp_core::SimLimits;
use cp_net::NetGraph;
use cp_save::*;
use cp_sim::SimRunner;
use cp_terrain::Terrain;
use cp_traffic::{Access, Agents, RunPoint, RunState, ServiceRuns, Side};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn the_v28_fixture_loads_and_round_trips_on_the_new_version() {
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v28-basic.sav")).unwrap();
    let (h, _) = read_header(&bytes).unwrap();
    assert_eq!(h.format_version, 28, "made by the v28 writer");
    let content = ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap();
    let l = load_with_map(&bytes, content.content_hash(), h.map_hash).unwrap();
    assert!(l.data.service_runs.is_empty());
    let again = save_with_map(&l.data, h.content_hash, &h.map_id, h.map_hash).unwrap();
    assert_eq!(
        read_header(&again).unwrap().0.format_version,
        FORMAT_VERSION
    );
    let l2 = load_with_map(&again, content.content_hash(), h.map_hash).unwrap();
    assert_eq!(l.data, l2.data);
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
fn a_load_with_a_parked_vehicle_keeps_the_checksum_and_the_vehicle_stays() {
    let content =
        Arc::new(ContentDb::load_dir(Path::new(ROOT).join("../../assets/content")).unwrap());
    let hash = content.content_hash();
    let bytes = std::fs::read(Path::new(ROOT).join("../../fixtures/saves/v28-basic.sav")).unwrap();
    let mut a = runner(&content);
    load_into_runner(&mut a, &bytes, hash).unwrap();
    let (seg, len) = {
        let net = a.world().resource::<NetGraph>();
        let (id, s) = net
            .segments()
            .find(|(_, s)| s.length >= 80.0 && content.road(s.road).class != RoadClass::Highway)
            .unwrap();
        (id, s.length)
    };
    let at = |s| {
        RunPoint::Access(Access {
            segment: seg,
            s,
            side: Side::Right,
        })
    };
    let run = a.world_mut().resource_mut::<ServiceRuns>().start_run(
        VehicleKind::Bus,
        at(len * 0.2),
        at(len * 0.8),
        true,
    );
    let mut parked = false;
    for _ in 0..3000 {
        a.tick_once();
        parked = matches!(
            a.world()
                .resource::<ServiceRuns>()
                .get(run)
                .map(|r| r.state),
            Some(RunState::Parked { .. })
        );
        if parked {
            break;
        }
    }
    assert!(parked, "the vehicle stops at its goal");
    let bytes = save_runner(&a, hash, "flatlands").unwrap();
    let mut b = runner(&content);
    load_into_runner(&mut b, &bytes, hash).unwrap();
    assert_eq!(a.checksum(), b.checksum());
    for _ in 0..200 {
        a.tick_once();
        b.tick_once();
    }
    assert_eq!(a.checksum(), b.checksum());
    assert!(
        matches!(
            b.world()
                .resource::<ServiceRuns>()
                .get(run)
                .map(|r| r.state),
            Some(RunState::Parked { .. })
        ),
        "still parked after the load"
    );
    assert_eq!(
        a.world().resource::<Agents>().vehicle_count(),
        b.world().resource::<Agents>().vehicle_count()
    );
}
